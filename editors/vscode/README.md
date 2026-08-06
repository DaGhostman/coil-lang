# Coil VS Code / Cursor extension

Minimal debug adapter wiring for the `coil` debug type. The extension launches
`coil-debug --dap` (stdio DAP) and forwards IDE breakpoints / stepping to the VM.

## Setup

```bash
cargo build   # builds coil-debug next to coil
cd editors/vscode
npm install
npm run compile
```

Install locally (VS Code):

```bash
code --install-extension .
```

In Cursor, use **Extensions: Install from VSIX** or open this folder and press F5
to run the extension in an Extension Development Host.

## Launch

Add to `.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "coil",
      "request": "launch",
      "name": "Coil: Launch current file",
      "program": "${file}",
      "cwd": "${workspaceFolder}"
    }
  ]
}
```

Set line breakpoints in `.hy` files, then F5.

## `adapterExecutable`

Override when `coil-debug` is not on `PATH` or not in `target/debug/`:

```json
"adapterExecutable": "/path/to/coil-debug"
```
