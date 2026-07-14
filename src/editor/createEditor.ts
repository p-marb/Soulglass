import { basicSetup } from 'codemirror'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import { searchKeymap } from '@codemirror/search'
import { autocompletion } from '@codemirror/autocomplete'
import { languageForPath } from './languages'
import { editorTheme } from './theme'
import type { ThemeDefinition } from '../settings/theme'

export interface CreateEditorOptions {
  parent: HTMLElement
  doc: string
  path: string
  theme: ThemeDefinition
  fontSize: number
  fontFamily: string
  onChange?: (value: string) => void
}

export function createEditor(options: CreateEditorOptions) {
  const updateListener = EditorView.updateListener.of((update) => {
    if (update.docChanged) options.onChange?.(update.state.doc.toString())
  })

  const extensions: Extension[] = [
    basicSetup,
    history(),
    autocompletion(),
    keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
    languageForPath(options.path),
    editorTheme(options.theme, options.fontSize, options.fontFamily),
    updateListener
  ]

  const state = EditorState.create({ doc: options.doc, extensions })
  return new EditorView({ state, parent: options.parent })
}
