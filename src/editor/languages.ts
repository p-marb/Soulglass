import { javascript } from '@codemirror/lang-javascript'
import { json } from '@codemirror/lang-json'
import { css } from '@codemirror/lang-css'
import { html } from '@codemirror/lang-html'
import { markdown } from '@codemirror/lang-markdown'
import { rust } from '@codemirror/lang-rust'
import type { Extension } from '@codemirror/state'

export function languageForPath(path: string): Extension {
  const lower = path.toLowerCase()
  if (lower.endsWith('.ts') || lower.endsWith('.tsx')) return javascript({ typescript: true, jsx: lower.endsWith('.tsx') })
  if (lower.endsWith('.js') || lower.endsWith('.jsx') || lower.endsWith('.svelte')) return javascript({ jsx: true })
  if (lower.endsWith('.json')) return json()
  if (lower.endsWith('.css') || lower.endsWith('.scss')) return css()
  if (lower.endsWith('.html')) return html()
  if (lower.endsWith('.md')) return markdown()
  if (lower.endsWith('.rs')) return rust()
  return []
}
