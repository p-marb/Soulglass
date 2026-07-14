import { writable } from 'svelte/store'
import { defaultTheme, type ThemeDefinition } from './theme'

export interface AppSettings {
  theme: ThemeDefinition
  fontSize: number
  fontFamily: string
  sidebarVisible: boolean
}

const initial: AppSettings = {
  theme: defaultTheme,
  fontSize: 14,
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  sidebarVisible: true
}

export const settings = writable<AppSettings>(initial)
