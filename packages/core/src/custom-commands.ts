/**
 * Custom command definition API — API-compatible with just-bash's `defineCommand`.
 */

import type { CustomCommand, CommandContext, ExecResult } from './types.js';

/**
 * Define a custom command that can be registered with a Bash instance.
 *
 * @example
 * ```ts
 * const hello = defineCommand("hello", async (args, ctx) => {
 *   const name = args[0] || "world";
 *   return { stdout: `Hello, ${name}!\n`, stderr: "", exitCode: 0 };
 * });
 *
 * const bash = new Bash({ customCommands: [hello] });
 * await bash.exec("hello Alice"); // "Hello, Alice!\n"
 * ```
 */
export function defineCommand(
  name: string,
  execute: (args: string[], ctx: CommandContext) => Promise<ExecResult>,
): CustomCommand {
  return { name, execute };
}
