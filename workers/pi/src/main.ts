import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

import { runNatsWorker } from "./nats-worker.js";
import { DiagnosisOnlyPiRuntime, PiSdkRuntime, executePiJob, type PiRuntimeAdapter } from "./pi-runtime.js";
import { redactSecrets } from "./redaction.js";

export async function processLine(line: string, runtime: PiRuntimeAdapter): Promise<string> {
  try {
    const result = await executePiJob(JSON.parse(line), runtime);
    return JSON.stringify(result);
  } catch (error) {
    return JSON.stringify({ error: redactSecrets(error) });
  }
}

export async function main(
  runtime: PiRuntimeAdapter = process.env.PITOOLS_PI_ENABLE_SDK === "1"
    ? new PiSdkRuntime()
    : new DiagnosisOnlyPiRuntime(),
): Promise<void> {
  if (process.env.PITOOLS_PI_TRANSPORT === "nats") {
    await runNatsWorker(runtime, processLine);
    return;
  }
  const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of input) {
    if (!line.trim()) continue;
    process.stdout.write(`${await processLine(line, runtime)}\n`);
  }
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
