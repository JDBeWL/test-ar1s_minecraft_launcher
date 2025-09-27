import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

export const useSettingsStore = defineStore('settings', () => {
  const maxMemory = ref(4096)
  const totalMemoryMB = ref(0)
  
  async function loadSystemMemory() {
    try {
      const memoryBytes = await invoke('get_total_memory') as number
      totalMemoryMB.value = Math.round(memoryBytes / 1024 / 1024)
    } catch (err) {
      console.error('Failed to get total memory:', err)
    }
  }

  async function loadMaxMemory() {
    try {
      const memory = await invoke('load_config_key', { key: 'maxMemory' })
      if (memory) {
        maxMemory.value = parseInt(memory as string, 10)
      }
    } catch (err) {
      console.error('Failed to get max memory:', err)
    }
  }

  async function saveMaxMemory() {
    try {
      await invoke('save_config_key', { key: 'maxMemory', value: maxMemory.value.toString() })
    } catch (err) {
      console.error('Failed to set max memory:', err)
    }
  }

  return {
    maxMemory,
    totalMemoryMB,
    loadSystemMemory,
    loadMaxMemory,
    saveMaxMemory
  }
})