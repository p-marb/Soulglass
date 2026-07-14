import type { EditorPlugin } from '../../src/plugins/plugin-api'

const plugin: EditorPlugin = {
  id: 'word-count',
  name: 'Word Count',
  activate(ctx) {
    ctx.commands.register({
      id: 'wordCount.show',
      title: 'Show Word Count',
      run: () => {
        const words = ctx.editor.getText().trim().split(/\s+/).filter(Boolean).length
        ctx.window.toast(`${words} words`)
      }
    })
  }
}

export default plugin
