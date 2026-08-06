import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

/** Resolve coil-debug for launch: config override, then workspace target/, then PATH. */
function resolveCoilDebug(config: vscode.DebugConfiguration): string {
  if (typeof config.adapterExecutable === "string" && config.adapterExecutable.length > 0) {
    return config.adapterExecutable;
  }
  const ws = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (ws) {
    for (const sub of ["debug", "release"]) {
      const candidate = path.join(ws, "target", sub, "coil-debug");
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return "coil-debug";
}

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory("coil", {
      createDebugAdapterDescriptor(
        session: vscode.DebugSession,
        executable: vscode.DebugAdapterExecutable | undefined,
      ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
        if (executable) {
          return executable;
        }
        const coilDebug = resolveCoilDebug(session.configuration);
        return new vscode.DebugAdapterExecutable(coilDebug, ["--dap"]);
      },
    }),
  );
}

export function deactivate(): void {}
