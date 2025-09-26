import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import 'vuetify/styles'
import { createVuetify } from 'vuetify'
import * as components from 'vuetify/components'
import * as directives from 'vuetify/directives'
import '@mdi/font/css/materialdesignicons.css'

// 尝试修改WebView进程名称
try {
  // 修改用户代理
  Object.defineProperty(navigator, 'userAgent', {
    get: function() { return 'Ar1s Launcher WebView'; }
  });
  
  // 修改应用名称
  document.title = 'Ar1s Launcher';
  
  // 如果是Tauri环境，尝试使用Tauri API
  if (typeof window !== 'undefined' && '__TAURI__' in window) {
    console.log('在Tauri环境中设置WebView名称');
    // 这里可以添加Tauri特定的API调用
  }
} catch (e) {
  console.error('修改WebView进程名称失败:', e);
}

// 创建 Vuetify 实例
const vuetify = createVuetify({
  components,
  directives,
  theme: {
    defaultTheme: 'dark',
  },
})

// 创建 Pinia 实例
const pinia = createPinia()

// 创建路由
const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', component: () => import('./views/HomeView.vue') },
    { path: '/settings', component: () => import('./views/SettingsView.vue') },
    { path: '/download', component: () => import('./views/DownloadView.vue') },
  ],
})

// 创建并挂载应用
const app = createApp(App)
app.use(vuetify)
app.use(pinia)
app.use(router)
app.mount('#root')