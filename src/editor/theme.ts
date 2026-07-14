import { EditorView } from '@codemirror/view'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { tags } from '@lezer/highlight'
import type { Extension } from '@codemirror/state'
import type { ThemeDefinition } from '../settings/theme'

export function editorTheme(theme: ThemeDefinition, fontSize: number, fontFamily: string): Extension {
  const syntax = HighlightStyle.define([
    { tag: tags.keyword, color: theme.colors.accent },
    { tag: [tags.string, tags.special(tags.string)], color: '#c6e2a6' },
    { tag: [tags.number, tags.bool], color: '#ffcf9f' },
    { tag: [tags.comment], color: theme.colors.muted, fontStyle: 'italic' },
    { tag: [tags.function(tags.variableName), tags.definition(tags.variableName)], color: '#ffd6a5' },
    { tag: [tags.typeName, tags.className], color: '#a5d6ff' }
  ])

  return [
    EditorView.theme({
      '&': {
        height: '100%',
        backgroundColor: 'transparent',
        color: theme.colors.foreground,
        fontSize: `${fontSize}px`,
        fontFamily
      },
      '.cm-scroller': { fontFamily },
      '.cm-content': { padding: '20px 0' },
      '.cm-line': { padding: '0 22px' },
      '.cm-gutters': {
        backgroundColor: 'transparent',
        color: theme.colors.muted,
        border: 'none'
      },
      '.cm-activeLine': { backgroundColor: 'rgba(255,255,255,0.045)' },
      '.cm-activeLineGutter': { backgroundColor: 'rgba(255,255,255,0.045)' },
      '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': {
        backgroundColor: `${theme.colors.selection} !important`
      },
      '.cm-cursor': { borderLeftColor: theme.colors.foreground },
      '.cm-tooltip': {
        backgroundColor: theme.colors.panel,
        border: `1px solid ${theme.colors.border}`,
        backdropFilter: 'blur(18px)'
      }
    }, { dark: theme.type === 'dark' }),
    syntaxHighlighting(syntax)
  ]
}
