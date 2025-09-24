<script setup lang="ts">
import { ref } from "vue";
import { useDisplay } from 'vuetify'

const { mobile } = useDisplay()
const drawer = ref(!mobile.value)
</script>

<template>
  <v-app>
    <v-navigation-drawer v-model="drawer" temporary>
      <v-list>
        <v-list-item title="主页" value="home" to="/" prepend-icon="mdi-home"></v-list-item>
        <v-list-item title="下载" value="download" to="/download" prepend-icon="mdi-download"></v-list-item>
        <v-list-item title="设置" value="settings" to="/settings" prepend-icon="mdi-cog"></v-list-item>
      </v-list>
    </v-navigation-drawer>

    <v-app-bar>
      <v-app-bar-nav-icon @click="drawer = !drawer"></v-app-bar-nav-icon>
      <v-toolbar-title>Ar1s Minecraft 启动器</v-toolbar-title>
    </v-app-bar>

    <v-main>
      <router-view></router-view>
    </v-main>
  </v-app>
</template>

<style>
:root {
  color-scheme: light dark;
}
</style>