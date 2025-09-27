<script setup lang="ts">

import { Window } from '@tauri-apps/api/window'

const appWindow = Window.getCurrent()
const window = {
  minimize: () => appWindow.minimize(),
  toggleMaximize: async () => {
    const isMaximized = await appWindow.isMaximized()
    isMaximized ? appWindow.unmaximize() : appWindow.maximize()
  },
  close: () => appWindow.close()
}

</script>

<template>
  <v-app>
    <v-navigation-drawer rail expand-on-hover :mobile-breakpoint="0">
      <v-list>
        <v-list-item title="启动" value="home" to="/" prepend-icon="mdi-rocket-launch"></v-list-item>
        <v-list-item title="下载" value="download" to="/download" prepend-icon="mdi-download"></v-list-item>
        <v-list-item title="设置" value="settings" to="/settings" prepend-icon="mdi-cog"></v-list-item>
      </v-list>
    </v-navigation-drawer>

        <v-app-bar class="titlebar" data-tauri-drag-region>

      <v-toolbar-title>Ar1s Launcher</v-toolbar-title>
      <v-spacer></v-spacer>
      <v-btn icon data-tauri-no-drag @click="window.minimize()">
        <v-icon>mdi-window-minimize</v-icon>
      </v-btn>
      <v-btn icon data-tauri-no-drag @click="window.toggleMaximize()">
        <v-icon>mdi-window-maximize</v-icon>
      </v-btn>
      <v-btn icon data-tauri-no-drag @click="window.close()">
        <v-icon>mdi-close</v-icon>
      </v-btn>
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

/* Hide scrollbar while keeping scroll functionality */
::-webkit-scrollbar {
  display: none;
}

.titlebar .v-toolbar__content {
  pointer-events: none;
}

.titlebar .v-btn {
  pointer-events: auto;
}
</style>