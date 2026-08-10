import { connect as openConnection, type Socket } from "node:net";
import { connect as openTlsConnection, type TLSSocket } from "node:tls";

import type { PiRuntimeAdapter } from "./pi-runtime.js";

type NatsSocket = Socket | TLSSocket;

const REQUEST_SUBJECT = "pitools.pi.requests";

/**
 * Minimal NATS 1.x client for the isolated worker transport.
 *
 * Keeping this boundary dependency-free means the runner can remain a small
 * image. It handles the subset required by request/reply: INFO, CONNECT,
 * SUB, MSG, PING/PONG, and PUB. The Rust control plane remains the source of
 * truth for timeouts, request binding, path allowlists, and redaction.
 */
export async function runNatsWorker(
  runtime: PiRuntimeAdapter,
  processLineFn: (line: string, runtime: PiRuntimeAdapter) => Promise<string>,
): Promise<void> {
  const endpoint = parseEndpoint(process.env.NATS_URL ?? "nats://127.0.0.1:4222");
  const socket = await openSocket(endpoint);
  const parser = new NatsParser(socket, runtime, processLineFn, endpoint);
  socket.on("data", (chunk: Buffer) => parser.push(chunk));
  await new Promise<void>((resolve, reject) => {
    socket.once("close", () => resolve());
    socket.once("error", reject);
  });
}

type Endpoint = {
  host: string;
  port: number;
  tls: boolean;
  username?: string;
  password?: string;
};

function parseEndpoint(raw: string): Endpoint {
  const url = new URL(raw);
  if (url.protocol !== "nats:" && url.protocol !== "tls:") {
    throw new Error("PiTools NATS_URL must use nats: or tls:");
  }
  if (url.username && !url.password) {
    throw new Error("PiTools NATS_URL requires a password when a username is supplied");
  }
  return {
    host: url.hostname,
    port: Number(url.port || 4222),
    tls: url.protocol === "tls:",
    ...(url.username ? { username: decodeURIComponent(url.username) } : {}),
    ...(url.password ? { password: decodeURIComponent(url.password) } : {}),
  };
}

function openSocket(endpoint: Endpoint): Promise<NatsSocket> {
  return new Promise((resolve, reject) => {
    const options = { host: endpoint.host, port: endpoint.port };
    const socket = endpoint.tls ? openTlsConnection(options) : openConnection(options);
    socket.once("connect", () => resolve(socket));
    socket.once("secureConnect", () => resolve(socket));
    socket.once("error", reject);
  });
}

class NatsParser {
  private buffer = Buffer.alloc(0);
  private infoReceived = false;
  private readonly sid = "1";

  public constructor(
    private readonly socket: NatsSocket,
    private readonly runtime: PiRuntimeAdapter,
    private readonly process: (line: string, runtime: PiRuntimeAdapter) => Promise<string>,
    private readonly endpoint: Endpoint,
  ) {}

  public push(chunk: Buffer): void {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    this.drain();
  }

  private drain(): void {
    while (true) {
      if (this.buffer.subarray(0, 4).toString() === "MSG ") {
        if (!this.consumeMessage()) return;
        continue;
      }
      const separator = this.buffer.indexOf("\r\n");
      if (separator < 0) return;
      const line = this.buffer.subarray(0, separator).toString();
      this.buffer = this.buffer.subarray(separator + 2);
      if (line.startsWith("INFO ")) {
        this.infoReceived = true;
        this.socket.write(
          `CONNECT ${JSON.stringify({
            verbose: false,
            pedantic: false,
            lang: "node",
            version: "1.0",
            protocol: 1,
            echo: false,
            ...(this.authFields()),
          })}\r\nSUB ${REQUEST_SUBJECT} ${this.sid}\r\nPING\r\n`,
        );
      } else if (line === "PING") {
        this.socket.write("PONG\r\n");
      } else if (line === "-ERR") {
        this.socket.destroy(new Error("NATS worker connection was rejected"));
      }
    }
  }

  private consumeMessage(): boolean {
    const separator = this.buffer.indexOf("\r\n");
    if (separator < 0) return false;
    const parts = this.buffer.subarray(0, separator).toString().split(" ");
    const sizePart = parts.at(-1);
    const size = sizePart === undefined ? Number.NaN : Number(sizePart);
    if (!Number.isSafeInteger(size) || size < 0 || size > 256 * 1024) {
      this.socket.destroy(new Error("NATS message size is invalid"));
      return false;
    }
    const start = separator + 2;
    if (this.buffer.length < start + size + 2) return false;
    const payload = this.buffer.subarray(start, start + size);
    this.buffer = this.buffer.subarray(start + size + 2);
    const reply = parts.length === 5 ? parts[3] : undefined;
    if (reply === undefined) return true;
    void this.process(payload.toString("utf8"), this.runtime).then((result) => {
      const bytes = Buffer.from(result, "utf8");
      this.socket.write(`PUB ${reply} ${bytes.byteLength}\r\n`);
      this.socket.write(bytes);
      this.socket.write("\r\n");
    });
    return true;
  }

  private authFields(): Record<string, string> {
    if (!this.infoReceived) return {};
    const username = this.endpoint.username ?? process.env.PITOOLS_NATS_USERNAME;
    const password = this.endpoint.password ?? process.env.PITOOLS_NATS_PASSWORD;
    return username !== undefined && password !== undefined ? { user: username, pass: password } : {};
  }
}
