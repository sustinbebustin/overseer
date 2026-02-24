/**
 * CLI bridge - spawns `os` binary and parses JSON output
 * 
 * Configuration is passed via constructor, not environment variables.
 * This allows the Rust CLI to set paths correctly when spawning.
 */
import { spawn } from "node:child_process";
import { statSync } from "node:fs";
import path from "node:path";
import { CliError, CliTimeoutError, InvalidPathError } from "./types.js";

const CLI_TIMEOUT_MS = 30_000;

export interface CliConfig {
  /** Path to the os binary */
  cliPath: string;
  /** Base working directory for CLI commands */
  cwd: string;
  /** Pinned DB path for all CLI commands */
  dbPath: string;
}

export interface CallCliOptions {
  /** Optional per-call cwd override (absolute or relative to base cwd) */
  cwd?: string;
}

// Global config, set by main entry point
let config: CliConfig = {
  cliPath: "os",
  cwd: path.resolve(process.cwd()),
  dbPath: ".overseer/tasks.db",
};

/**
 * Resolve a cwd override against base cwd and validate it.
 */
export function resolveCliCwd(cwdOverride?: string): string {
  if (cwdOverride === undefined) {
    return config.cwd;
  }

  const resolved = path.resolve(config.cwd, cwdOverride);

  let stats;
  try {
    stats = statSync(resolved);
  } catch {
    throw new InvalidPathError(`Path does not exist: ${cwdOverride}`, cwdOverride);
  }

  if (!stats.isDirectory()) {
    throw new InvalidPathError(`Path is not a directory: ${cwdOverride}`, cwdOverride);
  }

  return resolved;
}

function parseCliErrorMessage(stderr: string, code: number | null): string {
  const trimmed = stderr.trim();
  if (trimmed.length === 0) {
    return `os exited with code ${code ?? "unknown"}`;
  }

  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (parsed !== null && typeof parsed === "object") {
      const maybeError = Reflect.get(parsed, "error");
      if (typeof maybeError === "string") {
        return maybeError;
      }
    }
  } catch {
    // Keep original stderr text if not JSON.
  }

  return trimmed;
}

function isWorkflowCommand(args: string[]): boolean {
  return args.length >= 2 && args[0] === "task" && (args[1] === "start" || args[1] === "complete");
}

function addWorkflowRepoHint(message: string, args: string[]): string {
  if (!isWorkflowCommand(args)) {
    return message;
  }

  const lower = message.toLowerCase();
  const isNotRepo = lower.includes("not in a repository") || lower.includes("not a repository");
  if (!isNotRepo || lower.includes("repopath") || lower.includes("--cwd")) {
    return message;
  }

  return `${message} For monorepo roots without VCS, pass repoPath to workflow calls (tasks.start/tasks.complete) or launch with --cwd <repo-path>.`;
}

/**
 * Configure the CLI bridge
 */
export function configureCli(newConfig: CliConfig): void {
  const baseCwd = path.resolve(newConfig.cwd);
  const dbPath = path.isAbsolute(newConfig.dbPath)
    ? newConfig.dbPath
    : path.resolve(baseCwd, newConfig.dbPath);
  config = {
    cliPath: newConfig.cliPath,
    cwd: baseCwd,
    dbPath,
  };
}

/**
 * Get current CLI config
 */
export function getCliConfig(): CliConfig {
  return config;
}

/**
 * Execute os CLI command with --json flag
 */
export async function callCli(args: string[], options?: CallCliOptions): Promise<unknown> {
  const resolvedCwd = resolveCliCwd(options?.cwd);

  return new Promise((resolve, reject) => {
    const proc = spawn(config.cliPath, [...args, "--db", config.dbPath, "--json"], {
      cwd: resolvedCwd,
      stdio: ["ignore", "pipe", "pipe"],
    });

    const timeout = setTimeout(() => {
      proc.kill("SIGTERM");
      reject(new CliTimeoutError());
    }, CLI_TIMEOUT_MS);

    let stdout = "";
    let stderr = "";

    proc.stdout.on("data", (chunk: Buffer) => {
      stdout += chunk.toString();
    });

    proc.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });

    proc.on("error", (err) => {
      clearTimeout(timeout);
      reject(new CliError(`Failed to spawn os: ${err.message}`, -1, ""));
    });

    proc.on("close", (code) => {
      clearTimeout(timeout);

      if (code !== 0) {
        const message = addWorkflowRepoHint(parseCliErrorMessage(stderr, code), args);
        reject(new CliError(message, code ?? -1, stderr));
        return;
      }

      try {
        const result: unknown = JSON.parse(stdout);
        resolve(result);
      } catch (err) {
        reject(
          new CliError(
            `Invalid JSON from os: ${err instanceof Error ? err.message : String(err)}`,
            0,
            stdout
          )
        );
      }
    });
  });
}
