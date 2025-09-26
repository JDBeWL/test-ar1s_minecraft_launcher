<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from '@tauri-apps/api/event';

// --- State ---
// 移除未使用的versions变量
const allVersions = ref<Array<any>>([]);
const selectedVersion = ref('');
const downloadSource = ref('bmcl'); // Default to BMCL
const loading = ref(false);
const downloadProgress = ref({
  progress: 0,
  total: 0,
  speed: 0,
  status: 'idle' // idle, downloading, completed, cancelled, error
});
const isDownloading = computed(() => downloadProgress.value.status === 'downloading');
const searchQuery = ref('');
const versionType = ref('all');
const sortOrder = ref('newest');
const itemsPerPage = 10;
const currentPage = ref(1);

// --- Methods ---

// Fetch versions from backend
async function fetchVersions() {
  try {
    loading.value = true;
    const result = await invoke('get_versions');
    if (result && (result as any).versions) {
      allVersions.value = (result as any).versions;
    } else {
      allVersions.value = [];
    }
  } catch (err) {
    console.error('获取版本列表失败:', err);
    alert('获取版本列表失败，请检查网络连接或稍后再试');
  } finally {
    loading.value = false;
  }
}

// Start a download
async function startDownload(versionId: string) {
  selectedVersion.value = versionId;
  downloadProgress.value.status = 'downloading'; // Only update status
  
  try {
    await invoke('download_version', { 
      versionId: selectedVersion.value,
      mirror: downloadSource.value === 'bmcl' ? 'bmcl' : undefined,
    });
    // Note: Success is now handled by the event listener
  } catch (err) {
    console.error('Failed to download version:', err);
    downloadProgress.value.status = 'error';
    const errorMessage = (err as any).message || String(err);
    alert(`下载失败: ${errorMessage}`);
  }
}

// Cancel a download
async function cancelDownload() {
  await emit('cancel-download');
  // The backend will confirm cancellation via the progress event
}

// --- Computed Properties ---

const filteredVersions = computed(() => {
  let filtered = allVersions.value.filter(version => {
    const matchesSearch = searchQuery.value === '' || version.id.toLowerCase().includes(searchQuery.value.toLowerCase());
    const matchesType = versionType.value === 'all' || version.type === versionType.value;
    return matchesSearch && matchesType;
  });

  if (sortOrder.value === 'newest') {
    filtered.sort((a, b) => new Date(b.releaseTime).getTime() - new Date(a.releaseTime).getTime());
  } else if (sortOrder.value === 'oldest') {
    filtered.sort((a, b) => new Date(a.releaseTime).getTime() - new Date(b.releaseTime).getTime());
  } else if (sortOrder.value === 'az') {
    filtered.sort((a, b) => a.id.localeCompare(b.id));
  } else if (sortOrder.value === 'za') {
    filtered.sort((a, b) => b.id.localeCompare(a.id));
  }
  
  return filtered;
});

const paginatedVersions = computed(() => {
  const start = (currentPage.value - 1) * itemsPerPage;
  const end = start + itemsPerPage;
  return filteredVersions.value.slice(start, end);
});

const totalPages = computed(() => {
  return Math.ceil(filteredVersions.value.length / itemsPerPage);
});

const progressPercentage = computed(() => {
  if (downloadProgress.value.total === 0) return 0;
  return (downloadProgress.value.progress / downloadProgress.value.total) * 100;
});


// --- Lifecycle Hooks ---

onMounted(async () => {
  await fetchVersions();
  
  await listen('download-progress', (event) => {
    const data = event.payload as any;
    downloadProgress.value = data; // The backend now sends the full state object

    if (data.status === 'completed') {
      alert(`版本 ${selectedVersion.value} 下载完成！`);
      selectedVersion.value = '';
    } else if (data.status === 'cancelled' || data.status === 'error') {
      selectedVersion.value = '';
    }
  });
});
</script>

<template>
  <v-container>
    <v-card>
      <v-card-title class="d-flex align-center">
        下载 Minecraft
        <v-spacer></v-spacer>
        <v-btn variant="text" icon="mdi-refresh" @click="fetchVersions" :disabled="isDownloading"></v-btn>
      </v-card-title>
      <v-card-text>
        <!-- Search and Filter -->
        <v-row>
          <v-col cols="12" md="6">
            <v-text-field
              v-model="searchQuery"
              label="搜索版本"
              prepend-inner-icon="mdi-magnify"
              clearable
              hide-details
              :disabled="isDownloading"
              @update:model-value="currentPage = 1"
            ></v-text-field>
          </v-col>
          <v-col cols="6" md="3">
            <v-select
              v-model="versionType"
              label="版本类型"
              :items="[
                { title: '全部', value: 'all' },
                { title: '正式版', value: 'release' },
                { title: '快照版', value: 'snapshot' }
              ]"
              hide-details
              :disabled="isDownloading"
              @update:model-value="currentPage = 1"
            ></v-select>
          </v-col>
          <v-col cols="6" md="3">
            <v-select
              v-model="sortOrder"
              label="排序方式"
              :items="[
                { title: '最新优先', value: 'newest' },
                { title: '最旧优先', value: 'oldest' }
              ]"
              hide-details
              :disabled="isDownloading"
            ></v-select>
          </v-col>
        </v-row>
        
        <!-- Download Source / Progress Bar -->
        <v-row class="mt-4" :style="{ minHeight: '80px' }">
          <v-col v-if="!isDownloading">
            <v-card-subtitle>下载源</v-card-subtitle>
            <v-radio-group v-model="downloadSource" inline hide-details>
              <v-radio label="官方源" value="official"></v-radio>
              <v-radio label="BMCL 镜像" value="bmcl"></v-radio>
            </v-radio-group>
          </v-col>
          <v-col v-else>
             <v-card-subtitle>正在下载: {{ selectedVersion }}</v-card-subtitle>
            <v-progress-linear
              :model-value="progressPercentage"
              height="20"
              striped
              color="primary"
              class="mt-2"
            >
              <template v-slot:default="{ value }">
                <strong>{{ Math.ceil(value) }}%</strong>
              </template>
            </v-progress-linear>
            <div class="d-flex justify-space-between text-caption mt-1">
              <span>{{ downloadProgress.progress }} / {{ downloadProgress.total }} 个文件</span>
              <span>{{ downloadProgress.speed.toFixed(2) }} KB/s</span>
            </div>
          </v-col>
        </v-row>

        <!-- Versions Table -->
        <v-row>
          <v-col cols="12">
            <v-data-table
              :headers="[
                { title: '版本', key: 'id', align: 'start' },
                { title: '类型', key: 'type', align: 'center' },
                { title: '发布日期', key: 'releaseTime', align: 'end' },
                { title: '操作', key: 'actions', align: 'end', sortable: false }
              ]"
              :items="paginatedVersions"
              :loading="loading"
              items-per-page="-1"
              hide-default-footer
            >
              <template v-slot:item.releaseTime="{ item }">
                {{ new Date(item.releaseTime).toLocaleDateString() }}
              </template>
              
              <template v-slot:item.actions="{ item }">
                <v-btn
                  v-if="isDownloading && selectedVersion === item.id"
                  color="error"
                  variant="tonal"
                  size="small"
                  @click="cancelDownload"
                >
                  取消
                </v-btn>
                <v-btn
                  v-else
                  color="primary"
                  variant="tonal"
                  size="small"
                  :disabled="isDownloading"
                  @click="startDownload(item.id)"
                >
                  下载
                </v-btn>
              </template>
              
              <template v-slot:no-data>
                <div class="text-center pa-4">
                  <p v-if="loading">正在加载...</p>
                  <p v-else>没有找到匹配的版本</p>
                </div>
              </template>
            </v-data-table>
            
            <v-pagination
              v-if="totalPages > 1"
              v-model="currentPage"
              :length="totalPages"
              :disabled="isDownloading"
              :total-visible="7"
              class="mt-4"
            ></v-pagination>
          </v-col>
        </v-row>
      </v-card-text>
    </v-card>
  </v-container>
</template>

<style scoped>
/* Keeping scoped styles minimal as Vuetify handles most of it */
</style>