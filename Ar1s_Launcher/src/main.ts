import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import 'vuetify/styles'
import { createVuetify } from 'vuetify'
import * as components from 'vuetify/components'
import * as directives from 'vuetify/directives'
import '@mdi/font/css/materialdesignicons.css'

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