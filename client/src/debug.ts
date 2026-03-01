import * as fs from "fs";
import * as net from "net";
import { EventEmitter } from "events";
import * as vscode from "vscode";
import {
  DebugSession,
  InitializedEvent,
  StoppedEvent,
  TerminatedEvent,
  Thread,
  StackFrame,
  Scope,
  Variable,
  OutputEvent,
  Breakpoint,
} from "@vscode/debugadapter";
import { DebugProtocol } from "@vscode/debugadapter/lib/debugProtocol";

// ── Protocol Constants ──────────────────────────────────────────────

// Debugger → BR
const MSG_FORCE_BREAK = 1;
const MSG_ISSUE_COMMAND = 2;

// BR → Debugger
const MSG_FORCE_BREAK_RESPONSE = 101;
const MSG_ENTER_INPUT_MODE = 102;
const MSG_COMMAND_RESPONSE = 103;
const MSG_DEBUG_DATA = 104;
const MSG_DEBUG_VAR_BREAK = 105;
const MSG_DEBUG_FUNC_BREAK = 106;
const MSG_DEBUG_LINE_BREAK = 107;
const MSG_DEBUG_BEGIN_BREAK = 108;
const MSG_SYNTAX_ERROR_INDENT = 109;
const MSG_DEBUG_MSG_LOG = 110;

// Both
const MSG_TERMINATE = 200;

const THREAD_ID = 1;

// ── Connection State Machine ────────────────────────────────────────

const enum ConnState {
  DISCONNECTED,
  LISTENING,
  CONNECTED,
  READY,
  BUSY,
  WAITING_INPUT,
  TERMINATED,
}

// ── BrConnection ────────────────────────────────────────────────────

interface QueuedCommand {
  command: string;
  resolve: (result: number) => void;
  reject: (err: Error) => void;
}

class BrConnection extends EventEmitter {
  private server: net.Server | undefined;
  private socket: net.Socket | undefined;
  private accepted = false;
  private buffer = Buffer.alloc(0);
  private state: ConnState = ConnState.DISCONNECTED;
  private commandQueue: QueuedCommand[] = [];
  private currentCommand: QueuedCommand | undefined;

  /** Start a TCP server and wait for BR to connect. Resolves with the actual port. */
  listen(host: string, port: number): Promise<number> {
    return new Promise((resolve, reject) => {
      this.state = ConnState.LISTENING;
      this.server = net.createServer((socket) => {
        if (this.accepted) {
          // Only one BR session at a time
          socket.destroy();
          return;
        }
        this.accepted = true;
        this.socket = socket;
        this.state = ConnState.CONNECTED;
        this.emit("connected");

        socket.on("data", (data) => {
          this.buffer = Buffer.concat([this.buffer, data]);
          this.processBuffer();
        });

        socket.on("error", (err) => {
          this.emit("error", err);
        });

        socket.on("close", () => {
          this.state = ConnState.TERMINATED;
          this.rejectPending(new Error("Connection closed"));
          this.emit("close");
        });
      });

      this.server.on("error", (err) => {
        reject(err);
      });

      this.server.listen(port, host, () => {
        const addr = this.server!.address() as net.AddressInfo;
        resolve(addr.port);
      });
    });
  }

  disconnect(): void {
    if (this.socket) {
      this.socket.destroy();
      this.socket = undefined;
    }
    if (this.server) {
      this.server.close();
      this.server = undefined;
    }
    this.state = ConnState.TERMINATED;
    this.rejectPending(new Error("Disconnected"));
  }

  sendForceBreak(): void {
    this.sendPacket(MSG_FORCE_BREAK);
  }

  sendCommand(command: string): Promise<number> {
    return new Promise((resolve, reject) => {
      this.commandQueue.push({ command, resolve, reject });
      this.drainQueue();
    });
  }

  sendTerminate(): void {
    this.sendPacket(MSG_TERMINATE);
  }

  getState(): ConnState {
    return this.state;
  }

  private sendPacket(typeId: number, payload?: Buffer): void {
    if (!this.socket || this.socket.destroyed) return;
    const payloadLen = payload ? payload.length : 0;
    const totalLen = 8 + payloadLen; // 4 bytes length + 4 bytes type + payload
    const header = Buffer.alloc(8);
    header.writeUInt32BE(totalLen, 0);
    header.writeUInt32BE(typeId, 4);
    if (payload) {
      this.socket.write(Buffer.concat([header, payload]));
    } else {
      this.socket.write(header);
    }
  }

  private drainQueue(): void {
    if (this.state !== ConnState.READY || this.currentCommand) return;
    const next = this.commandQueue.shift();
    if (!next) {
      this.emit("idle");
      return;
    }
    this.currentCommand = next;
    this.state = ConnState.BUSY;
    const payload = Buffer.from(next.command, "latin1");
    this.sendPacket(MSG_ISSUE_COMMAND, payload);
  }

  private processBuffer(): void {
    while (this.buffer.length >= 4) {
      const packetLen = this.buffer.readUInt32BE(0);
      if (packetLen < 8) {
        // Invalid packet, skip 4 bytes
        this.buffer = this.buffer.subarray(4);
        continue;
      }
      if (this.buffer.length < packetLen) break; // Wait for more data

      const typeId = this.buffer.readUInt32BE(4);
      const payload = this.buffer.subarray(8, packetLen);
      this.buffer = this.buffer.subarray(packetLen);

      this.handlePacket(typeId, payload);
    }
  }

  private handlePacket(typeId: number, payload: Buffer): void {
    switch (typeId) {
      case MSG_FORCE_BREAK_RESPONSE:
        this.emit("forceBreakResponse");
        break;

      case MSG_ENTER_INPUT_MODE: {
        const status = payload.length >= 4 ? payload.readUInt32BE(0) : 0;
        const errorCode = payload.length >= 8 ? payload.readUInt32BE(4) : 0;
        const wasConnected = this.state === ConnState.CONNECTED;
        this.state = ConnState.READY;
        if (wasConnected) {
          this.emit("ready");
        }
        this.emit("enterInputMode", status, errorCode);
        this.drainQueue();
        break;
      }

      case MSG_COMMAND_RESPONSE: {
        const resultCode = payload.length >= 4 ? payload.readUInt32BE(0) : 0;
        this.state = ConnState.WAITING_INPUT;
        if (this.currentCommand) {
          const cmd = this.currentCommand;
          this.currentCommand = undefined;
          cmd.resolve(resultCode);
        }
        this.emit("commandResponse", resultCode);
        break;
      }

      case MSG_DEBUG_DATA: {
        const channel = payload.length >= 4 ? payload.readUInt32BE(0) : 0;
        const text = payload.length > 4 ? payload.subarray(4).toString("latin1") : "";
        this.emit("debugData", channel, text);
        break;
      }

      case MSG_DEBUG_VAR_BREAK: {
        const name = payload.toString("latin1");
        this.emit("debugVarBreak", name);
        break;
      }

      case MSG_DEBUG_FUNC_BREAK: {
        const name = payload.toString("latin1");
        this.emit("debugFuncBreak", name);
        break;
      }

      case MSG_DEBUG_LINE_BREAK: {
        const line = payload.length >= 4 ? payload.readUInt32BE(0) : 0;
        const clause = payload.length >= 8 ? payload.readUInt32BE(4) : 0;
        this.emit("debugLineBreak", line, clause);
        break;
      }

      case MSG_DEBUG_BEGIN_BREAK:
        this.emit("debugBeginBreak");
        break;

      case MSG_SYNTAX_ERROR_INDENT: {
        const index = payload.length >= 4 ? payload.readUInt32BE(0) : 0;
        this.emit("syntaxErrorIndent", index);
        break;
      }

      case MSG_DEBUG_MSG_LOG: {
        const source = payload.length >= 4 ? payload.readUInt32BE(0) : 0;
        const level = payload.length >= 8 ? payload.readUInt32BE(4) : 0;
        const message = payload.length > 8 ? payload.subarray(8).toString("latin1") : "";
        this.emit("debugMsgLog", source, level, message);
        break;
      }

      case MSG_TERMINATE:
        this.state = ConnState.TERMINATED;
        this.rejectPending(new Error("BR terminated connection"));
        this.emit("terminate");
        break;
    }
  }

  private rejectPending(err: Error): void {
    if (this.currentCommand) {
      this.currentCommand.reject(err);
      this.currentCommand = undefined;
    }
    for (const cmd of this.commandQueue) {
      cmd.reject(err);
    }
    this.commandQueue = [];
  }
}

// ── Line Map Helpers ────────────────────────────────────────────────

interface LineMap {
  /** Map from BR line number → editor line (1-based) */
  brToEditor: Map<number, number>;
  /** Map from editor line (1-based) → BR line number */
  editorToBr: Map<number, number>;
}

function buildLineMap(sourcePath: string): LineMap {
  const brToEditor = new Map<number, number>();
  const editorToBr = new Map<number, number>();

  let content: string;
  try {
    content = fs.readFileSync(sourcePath, "latin1");
  } catch {
    return { brToEditor, editorToBr };
  }

  const lines = content.split(/\r?\n/);
  const lineNumRe = /^\s*(\d{3,5})\s/;

  for (let i = 0; i < lines.length; i++) {
    const match = lineNumRe.exec(lines[i]);
    if (match) {
      const brLine = parseInt(match[1], 10);
      const editorLine = i + 1; // 1-based
      brToEditor.set(brLine, editorLine);
      editorToBr.set(editorLine, brLine);
    }
  }

  return { brToEditor, editorToBr };
}

// ── BrDebugSession ──────────────────────────────────────────────────

interface AttachArgs extends DebugProtocol.AttachRequestArguments {
  host?: string;
  port: number;
  stopOnAttach?: boolean;
}

export class BrDebugSession extends DebugSession {
  private connection = new BrConnection();

  // Stop state
  private stopLine = 0;
  private stopClause = 0;
  private stopFunction = "";
  private debugBreakPending = false;
  private stopReason = "breakpoint";

  // Breakpoints
  private breakpointsBySource = new Map<string, Map<number, number>>(); // source path → (editor line → BP id)
  private nextBpId = 1;
  private lineMapBySource = new Map<string, LineMap>();

  // Variables collected from DEBUG_DATA
  private collectedVariables: Variable[] = [];

  // Stack trace from STATUS STACK
  private cachedStackFrames: StackFrame[] = [];

  // Config
  private attachArgs: AttachArgs | undefined;

  public constructor() {
    super();
    this.setDebuggerLinesStartAt1(true);
    this.setDebuggerColumnsStartAt1(true);
  }

  // ── DAP: initialize ───────────────────────────────────────────

  protected initializeRequest(
    response: DebugProtocol.InitializeResponse,
    _args: DebugProtocol.InitializeRequestArguments,
  ): void {
    response.body = response.body || {};
    response.body.supportsConfigurationDoneRequest = true;
    response.body.supportsEvaluateForHovers = false;
    response.body.supportsTerminateRequest = true;
    response.body.supportsCancelRequest = false;
    response.body.supportsBreakpointLocationsRequest = false;

    this.sendResponse(response);
    this.sendEvent(new InitializedEvent());
  }

  // ── DAP: attach ───────────────────────────────────────────────

  protected async attachRequest(
    response: DebugProtocol.AttachResponse,
    args: AttachArgs,
  ): Promise<void> {
    this.attachArgs = args;
    const host = args.host || "127.0.0.1";
    const port = args.port;

    this.setupConnectionEvents();

    let actualPort: number;
    try {
      actualPort = await this.connection.listen(host, port);
    } catch (err: any) {
      response.success = false;
      response.message = `Failed to start debug server on ${host}:${port}: ${err.message}`;
      this.sendResponse(response);
      return;
    }

    this.sendEvent(
      new OutputEvent(
        `Listening on ${host}:${actualPort} — run DEBUG CONNECT ${host}:${actualPort} in BR\n`,
        "console",
      ),
    );

    this.sendResponse(response);
  }

  // ── DAP: configurationDone ────────────────────────────────────

  protected configurationDoneRequest(
    response: DebugProtocol.ConfigurationDoneResponse,
    _args: DebugProtocol.ConfigurationDoneArguments,
  ): void {
    this.sendResponse(response);
  }

  // ── DAP: setBreakpoints ───────────────────────────────────────

  protected async setBreakPointsRequest(
    response: DebugProtocol.SetBreakpointsResponse,
    args: DebugProtocol.SetBreakpointsArguments,
  ): Promise<void> {
    const sourcePath = args.source.path || "";
    const requestedLines = args.breakpoints?.map((bp) => bp.line) || [];

    // Build or retrieve line map
    if (!this.lineMapBySource.has(sourcePath)) {
      this.lineMapBySource.set(sourcePath, buildLineMap(sourcePath));
    }
    const lineMap = this.lineMapBySource.get(sourcePath)!;

    // Get previously set breakpoints for this source
    const previousBps = this.breakpointsBySource.get(sourcePath) || new Map();
    const newBps = new Map<number, number>();
    const resultBreakpoints: Breakpoint[] = [];

    // Clear breakpoints that are no longer requested
    for (const [editorLine] of previousBps) {
      if (!requestedLines.includes(editorLine)) {
        const brLine = lineMap.editorToBr.get(editorLine);
        if (brLine !== undefined) {
          this.queueCommand(`BREAK CLEAR ${brLine}`);
        }
      }
    }

    // Set new breakpoints
    for (const editorLine of requestedLines) {
      const brLine = lineMap.editorToBr.get(editorLine);
      if (brLine !== undefined) {
        // Only send BREAK command if this is a new breakpoint
        if (!previousBps.has(editorLine)) {
          this.queueCommand(`BREAK ${brLine}`);
        }
        const bpId = previousBps.get(editorLine) || this.nextBpId++;
        newBps.set(editorLine, bpId);
        const bp = new Breakpoint(true, editorLine);
        bp.setId(bpId);
        resultBreakpoints.push(bp);
      } else {
        // No BR line number on this editor line — breakpoint unverified
        const bpId = this.nextBpId++;
        const bp = new Breakpoint(false, editorLine);
        bp.setId(bpId);
        resultBreakpoints.push(bp);
      }
    }

    this.breakpointsBySource.set(sourcePath, newBps);

    response.body = { breakpoints: resultBreakpoints };
    this.sendResponse(response);
  }

  // ── DAP: threads ──────────────────────────────────────────────

  protected threadsRequest(response: DebugProtocol.ThreadsResponse): void {
    response.body = {
      threads: [new Thread(THREAD_ID, "BR Main")],
    };
    this.sendResponse(response);
  }

  // ── DAP: stackTrace ───────────────────────────────────────────

  protected async stackTraceRequest(
    response: DebugProtocol.StackTraceResponse,
    _args: DebugProtocol.StackTraceArguments,
  ): Promise<void> {
    // Request stack trace from BR
    const stackText = await this.sendCommandAndCollect("STATUS STACK >DEBUG:");

    const frames: StackFrame[] = [];
    if (stackText.trim()) {
      const lines = stackText.trim().split(/\r?\n/);
      for (let i = 0; i < lines.length; i++) {
        const line = lines[i].trim();
        if (!line) continue;

        // Parse stack trace lines — format varies but typically:
        // "ProgramName LINE nnn" or "FunctionName LINE nnn"
        const lineMatch = /(?:LINE\s+)?(\d+)/i.exec(line);
        const lineNum = lineMatch ? parseInt(lineMatch[1], 10) : 0;

        const frame = new StackFrame(
          i,
          line,
          undefined, // Source would require knowing the file path
          lineNum,
        );
        frames.push(frame);
      }
    }

    // If we have no stack frames from BR, create a synthetic one from stop position
    if (frames.length === 0) {
      const name = this.stopFunction || `Line ${this.stopLine}`;
      const frame = new StackFrame(0, name, undefined, this.stopLine);
      frames.push(frame);
    }

    this.cachedStackFrames = frames;
    response.body = {
      stackFrames: frames,
      totalFrames: frames.length,
    };
    this.sendResponse(response);
  }

  // ── DAP: scopes ───────────────────────────────────────────────

  protected scopesRequest(
    response: DebugProtocol.ScopesResponse,
    _args: DebugProtocol.ScopesArguments,
  ): void {
    response.body = {
      scopes: [new Scope("Locals", 1, false)],
    };
    this.sendResponse(response);
  }

  // ── DAP: variables ────────────────────────────────────────────

  protected variablesRequest(
    response: DebugProtocol.VariablesResponse,
    _args: DebugProtocol.VariablesArguments,
  ): void {
    response.body = {
      variables: this.collectedVariables,
    };
    this.sendResponse(response);
  }

  // ── DAP: evaluate (Debug Console) ─────────────────────────────

  protected async evaluateRequest(
    response: DebugProtocol.EvaluateResponse,
    args: DebugProtocol.EvaluateArguments,
  ): Promise<void> {
    const expr = args.expression.trim();
    if (!expr) {
      response.body = { result: "", variablesReference: 0 };
      this.sendResponse(response);
      return;
    }

    const result = await this.sendCommandAndCollect(expr);

    response.body = {
      result: result || "(no output)",
      variablesReference: 0,
    };
    this.sendResponse(response);
  }

  // ── DAP: continue ─────────────────────────────────────────────

  protected async continueRequest(
    response: DebugProtocol.ContinueResponse,
    _args: DebugProtocol.ContinueArguments,
  ): Promise<void> {
    this.collectedVariables = [];
    this.queueCommand("CONTINUE");
    response.body = { allThreadsContinued: true };
    this.sendResponse(response);
  }

  // ── DAP: next (step over) ────────────────────────────────────

  protected async nextRequest(
    response: DebugProtocol.NextResponse,
    _args: DebugProtocol.NextArguments,
  ): Promise<void> {
    this.collectedVariables = [];
    this.stopReason = "step";
    this.queueCommand("DEBUG STEP OVER LINE");
    this.sendResponse(response);
  }

  // ── DAP: stepIn ───────────────────────────────────────────────

  protected async stepInRequest(
    response: DebugProtocol.StepInResponse,
    _args: DebugProtocol.StepInArguments,
  ): Promise<void> {
    this.collectedVariables = [];
    this.stopReason = "step";
    this.queueCommand("DEBUG STEP INTO LINE");
    this.sendResponse(response);
  }

  // ── DAP: stepOut ──────────────────────────────────────────────

  protected async stepOutRequest(
    response: DebugProtocol.StepOutResponse,
    _args: DebugProtocol.StepOutArguments,
  ): Promise<void> {
    this.collectedVariables = [];
    this.stopReason = "step";
    // BR has no step-out; use CONTINUE as fallback
    this.queueCommand("CONTINUE");
    this.sendResponse(response);
  }

  // ── DAP: pause ────────────────────────────────────────────────

  protected pauseRequest(
    response: DebugProtocol.PauseResponse,
    _args: DebugProtocol.PauseArguments,
  ): void {
    this.stopReason = "pause";
    this.connection.sendForceBreak();
    this.sendResponse(response);
  }

  // ── DAP: disconnect ───────────────────────────────────────────

  protected disconnectRequest(
    response: DebugProtocol.DisconnectResponse,
    _args: DebugProtocol.DisconnectArguments,
  ): void {
    try {
      this.connection.sendTerminate();
    } catch {
      // ignore if already disconnected
    }
    this.connection.disconnect();
    this.sendResponse(response);
  }

  // ── DAP: terminate ────────────────────────────────────────────

  protected terminateRequest(
    response: DebugProtocol.TerminateResponse,
    _args: DebugProtocol.TerminateArguments,
  ): void {
    try {
      this.connection.sendTerminate();
    } catch {
      // ignore
    }
    this.connection.disconnect();
    this.sendResponse(response);
  }

  // ── Connection Event Wiring ───────────────────────────────────

  private setupConnectionEvents(): void {
    this.connection.on("connected", () => {
      this.sendEvent(new OutputEvent("BR connected\n", "console"));
    });

    this.connection.on("ready", () => {
      this.sendEvent(new OutputEvent("Debug session active\n", "console"));
      if (this.attachArgs?.stopOnAttach !== false) {
        this.connection.sendForceBreak();
      }
    });

    this.connection.on("enterInputMode", (_status: number, _errorCode: number) => {
      if (this.debugBreakPending) {
        this.debugBreakPending = false;
        // Emit StoppedEvent once the command queue drains.
        // drainQueue() is called right after this handler returns (in handlePacket),
        // and emits "idle" if the queue is empty.
        this.connection.once("idle", () => {
          this.sendEvent(new StoppedEvent(this.stopReason, THREAD_ID));
          this.stopReason = "breakpoint";
        });
      }
    });

    this.connection.on("debugLineBreak", (line: number, clause: number) => {
      this.stopLine = line;
      this.stopClause = clause;
      this.stopReason = "breakpoint";
    });

    this.connection.on("debugFuncBreak", (name: string) => {
      this.stopFunction = name;
    });

    this.connection.on("debugBeginBreak", () => {
      this.debugBreakPending = true;
    });

    this.connection.on("forceBreakResponse", () => {
      // Force break acknowledged — stop will come via debugBeginBreak + enterInputMode
    });

    this.connection.on("debugData", (_channel: number, text: string) => {
      // Accumulate as output and as variables
      const trimmed = text.trim();
      if (trimmed) {
        this.sendEvent(new OutputEvent(text, "stdout"));
        // Try to parse "name = value" format
        const eqIdx = trimmed.indexOf("=");
        if (eqIdx > 0) {
          const name = trimmed.substring(0, eqIdx).trim();
          const value = trimmed.substring(eqIdx + 1).trim();
          this.collectedVariables.push(new Variable(name, value));
        } else {
          // Store as generic result
          this.collectedVariables.push(
            new Variable(`result_${this.collectedVariables.length}`, trimmed),
          );
        }
      }
    });

    this.connection.on("debugMsgLog", (_source: number, _level: number, message: string) => {
      this.sendEvent(new OutputEvent(`[BR] ${message}\n`, "console"));
    });

    this.connection.on("terminate", () => {
      this.sendEvent(new TerminatedEvent());
    });

    this.connection.on("error", (err: Error) => {
      this.sendEvent(new OutputEvent(`Connection error: ${err.message}\n`, "stderr"));
    });

    this.connection.on("close", () => {
      this.sendEvent(new TerminatedEvent());
    });
  }

  // ── Helpers ───────────────────────────────────────────────────

  private queueCommand(command: string): void {
    this.connection.sendCommand(command).catch((err) => {
      this.sendEvent(new OutputEvent(`Command failed: ${err.message}\n`, "stderr"));
    });
  }

  /** Send a command and collect all DEBUG_DATA output until COMMAND_RESPONSE */
  private sendCommandAndCollect(command: string): Promise<string> {
    return new Promise((resolve) => {
      let collected = "";

      const onData = (_channel: number, text: string) => {
        collected += text;
      };

      this.connection.on("debugData", onData);

      this.connection.sendCommand(command).then(
        () => {
          this.connection.removeListener("debugData", onData);
          resolve(collected);
        },
        (err) => {
          this.connection.removeListener("debugData", onData);
          resolve(`Error: ${err.message}`);
        },
      );
    });
  }
}

// ── Activation ──────────────────────────────────────────────────────

export function activateDebug(context: vscode.ExtensionContext): void {
  const factory: vscode.DebugAdapterDescriptorFactory = {
    createDebugAdapterDescriptor(_session) {
      return new vscode.DebugAdapterInlineImplementation(new BrDebugSession());
    },
  };
  context.subscriptions.push(vscode.debug.registerDebugAdapterDescriptorFactory("br", factory));
}
