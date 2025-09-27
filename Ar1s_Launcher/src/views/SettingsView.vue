<script setup lang="ts">
import { ref, onMounted, computed } from "vue";

function formatJavaPath(rawPath: string) {
  if (!rawPath) return '';
  // 统一转换为正斜杠显示
  return rawPath.replace(/\\/g, '/');
}
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from '@tauri-apps/api/event';
import { useSettingsStore } from '../stores/settings';

const settingsStore = useSettingsStore()
const gameDir = ref('')
const versionIsolation = ref(true)
const javaPath = ref('')
const isJavaPathValid = ref(false)
const javaInstallations = ref<string[]>([])
const loadingJava = ref(false)
const downloadThreads = ref(8);

const totalMemoryGB = computed(() => (settingsStore.totalMemoryMB / 1024).toFixed(1));

// 内存相关函数已迁移到Pinia store

// 加载已保存的游戏目录
async function loadGameDir() {
  try {
    const dir = await invoke('get_game_dir');
    gameDir.value = dir as string;
  } catch (err) {
    console.error('Failed to get game directory:', err);
  }
}

// 加载已保存的Java路径
async function loadJavaPath() {
  try {
    javaPath.value = (await invoke('load_config_key', { key: 'javaPath' })) as string;
    isJavaPathValid.value = await invoke('validate_java_path', { path: javaPath.value });
  } catch (error) {
    console.error('Failed to load Java path:', error);
  }
}

// 查找系统中的Java安装
async function findJavaInstallations() {
  try {
    loadingJava.value = true;
    const installations = await invoke('find_java_installations_command');
    javaInstallations.value = installations as string[];
    
    // 如果找到了Java安装但还没有设置Java路径，则自动选择第一个
    if (javaInstallations.value.length > 0 && !javaPath.value) {
      javaPath.value = javaInstallations.value[0];
      await setJavaPath(javaPath.value);
    }
    
    loadingJava.value = false;
  } catch (err) {
    console.error('Failed to find Java installations:', err);
    loadingJava.value = false;
  }
}

// 设置Java路径
async function setJavaPath(path: string) {
  try {
    await invoke('save_config_key', { key: 'javaPath', value: path });
  } catch (err) {
    console.error('Failed to set Java path:', err);
  }
}

async function selectGameDir() {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: '选择游戏目录'
    });
    if (selected) {
      gameDir.value = selected as string;
      await invoke('set_game_dir', { path: gameDir.value, window: {} });
    }
  } catch (err) {
    console.error('Failed to select directory:', err);
  }
}

// 获取和保存下载线程数
async function loadDownloadThreads() {
  try {
    const threads = await invoke('get_download_threads');
    downloadThreads.value = threads as number;
  } catch (err) {
    console.error('Failed to get download threads:', err);
  }
}

async function saveDownloadThreads() {
  try {
    await invoke('set_download_threads', { threads: downloadThreads.value });
  } catch (err) {
    console.error('Failed to set download threads:', err);
  }
}

// 在组件挂载时加载所有设置
onMounted(async () => {
  await settingsStore.loadSystemMemory();
  await settingsStore.loadMaxMemory();
  await loadGameDir();
  await loadJavaPath();
  await findJavaInstallations();
  await loadDownloadThreads();
  
  // 监听游戏目录变更事件
  await listen('game-dir-changed', (event) => {
    gameDir.value = event.payload as string;
  });
});
</script>

<template>
  <v-container>
    <v-card>
      <v-card-title>设置</v-card-title>
      <v-card-text>
        <v-text-field
          v-model="gameDir"
          label="游戏目录"
          append-inner-icon="mdi-folder"
          @click:append-inner="selectGameDir"
          readonly
        ></v-text-field>

        <v-switch
          v-model="versionIsolation"
          label="版本隔离"
        ></v-switch>

        <v-row align="center" class="mt-4">
          <v-col cols="8">
            <v-slider
              v-model="settingsStore.maxMemory"
              label="最大内存 (MB)"
              :min="512"
              :max="Math.floor(settingsStore.totalMemoryMB * 0.8)"
              :step="128"
              thumb-label
              :hint="`可用范围: 512MB - ${Math.floor(settingsStore.totalMemoryMB * 0.8)}MB (80% of ${settingsStore.totalMemoryMB}MB)`"
              persistent-hint
              @end="settingsStore.saveMaxMemory"
            ></v-slider>
          </v-col>
          <v-col cols="4">
            <v-text-field
              v-model.number="settingsStore.maxMemory"
              type="number"
              label="内存大小"
              suffix="MB"
              :rules="[
                v => !!v || '必须输入内存大小',
                v => (v >= 512 && v <= Math.floor(settingsStore.totalMemoryMB * 0.8)) || `必须在512-${Math.floor(settingsStore.totalMemoryMB * 0.8)}MB之间`
              ]"
              density="compact"
              @change="settingsStore.saveMaxMemory"
            ></v-text-field>
          </v-col>
        </v-row>
        <div class="text-caption ml-2 mb-4">
          系统总内存: {{ settingsStore.totalMemoryMB }} MB (约 {{ totalMemoryGB }} GB)
        </div>


        <v-slider
          v-model="downloadThreads"
          label="下载线程数"
          class="mt-4"
          :min="1"
          :max="16"
          :step="1"
          thumb-label
          show-ticks="always"
          persistent-hint
          hint="设置多线程下载时使用的线程数量"
          @end="saveDownloadThreads"
        ></v-slider>

        <v-combobox
          v-model="javaPath"
          :items="javaInstallations.map(p => formatJavaPath(p))"
          label="Java 路径"
          class="mt-8"
          :loading="loadingJava"
          persistent-hint
          hint="选择或输入一个Java路径"
          @update:model-value="setJavaPath"
        >
          <template v-slot:append>
            <v-btn
              icon
              variant="text"
              :loading="loadingJava"
              @click="findJavaInstallations"
              title="自动查找Java安装"
            >
              <v-icon>mdi-refresh</v-icon>
            </v-btn>
          </template>
        </v-combobox>
      </v-card-text>
    </v-card>
  </v-container>
</template>
