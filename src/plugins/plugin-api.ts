export interface Command {
  id: string
  title: string
  run: () => void | Promise<void>
}

export class CommandRegistry {
  private commands = new Map<string, Command>()

  register(command: Command) {
    this.commands.set(command.id, command)
    return () => this.commands.delete(command.id)
  }

  list() {
    return [...this.commands.values()].sort((a, b) => a.title.localeCompare(b.title))
  }

  async run(id: string) {
    const command = this.commands.get(id)
    if (!command) throw new Error(`Command not found: ${id}`)
    await command.run()
  }
}

export interface EditorAPI {
  getText: () => string
  setText: (text: string) => void
  getPath: () => string
}

export interface WindowAPI {
  toast: (message: string) => void
}

export interface PluginContext {
  commands: CommandRegistry
  editor: EditorAPI
  window: WindowAPI
}

export interface EditorPlugin {
  id: string
  name: string
  activate: (ctx: PluginContext) => void | Promise<void>
  deactivate?: () => void | Promise<void>
}
