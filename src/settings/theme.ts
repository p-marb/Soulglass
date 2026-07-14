export type GlassMaterial = 'auto' | 'mica' | 'acrylic' | 'blur' | 'hud' | 'none'

export interface ThemeDefinition {
  name: string
  type: 'dark' | 'light'
  colors: {
    background: string
    foreground: string
    muted: string
    accent: string
    panel: string
    border: string
    editor: string
    selection: string
  }
  glass: {
    enabled: boolean
    material: GlassMaterial
    opacity: number
    blur: number
    saturation: number
    noise: number
  }
}

export const defaultTheme: ThemeDefinition = {
  name: 'Milk Glass Dark',
  type: 'dark',
  colors: {
    background: '#0d0d12',
    foreground: '#eeeae2',
    muted: '#9e9a91',
    accent: '#8fb8ff',
    panel: 'rgba(22, 22, 30, 0.62)',
    border: 'rgba(255,255,255,0.14)',
    editor: 'rgba(10,10,15,0.52)',
    selection: 'rgba(143,184,255,0.28)'
  },
  glass: {
    enabled: true,
    material: 'auto',
    opacity: 0.68,
    blur: 26,
    saturation: 1.35,
    noise: 0.035
  }
}

export function applyTheme(theme: ThemeDefinition) {
  const root = document.documentElement
  root.dataset.themeType = theme.type
  root.style.setProperty('--bg', theme.colors.background)
  root.style.setProperty('--text', theme.colors.foreground)
  root.style.setProperty('--muted', theme.colors.muted)
  root.style.setProperty('--accent', theme.colors.accent)
  root.style.setProperty('--panel', theme.colors.panel)
  root.style.setProperty('--border', theme.colors.border)
  root.style.setProperty('--editor-bg', theme.colors.editor)
  root.style.setProperty('--selection', theme.colors.selection)
  root.style.setProperty('--glass-opacity', String(theme.glass.opacity))
  root.style.setProperty('--glass-blur', `${theme.glass.blur}px`)
  root.style.setProperty('--glass-saturation', `${theme.glass.saturation * 100}%`)
  root.style.setProperty('--noise-opacity', String(theme.glass.noise))
}
