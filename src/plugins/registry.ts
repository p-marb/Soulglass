import { CommandRegistry, type PluginContext, type EditorAPI, type WindowAPI, type EditorPlugin } from './plugin-api'

export const commands = new CommandRegistry()

export function createPluginContext(editor: EditorAPI, window: WindowAPI): PluginContext {
  return { commands, editor, window }
}

export async function activateBuiltInPlugins(ctx: PluginContext) {
  const modules = import.meta.glob('../../plugins/*/index.ts')
  for (const load of Object.values(modules)) {
    const mod = await load() as { default: EditorPlugin }
    await mod.default.activate(ctx)
  }
}
