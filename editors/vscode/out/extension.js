"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const fs = __importStar(require("fs"));
const path = __importStar(require("path"));
const vscode = __importStar(require("vscode"));
/** Resolve coil-debug for launch: config override, then workspace target/, then PATH. */
function resolveCoilDebug(config) {
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
function activate(context) {
    context.subscriptions.push(vscode.debug.registerDebugAdapterDescriptorFactory("coil", {
        createDebugAdapterDescriptor(session, executable) {
            if (executable) {
                return executable;
            }
            const coilDebug = resolveCoilDebug(session.configuration);
            return new vscode.DebugAdapterExecutable(coilDebug, ["--dap"]);
        },
    }));
}
function deactivate() { }
//# sourceMappingURL=extension.js.map