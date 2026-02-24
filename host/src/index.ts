#!/usr/bin/env node
/**
 * Overseer Host - Unified entry point for MCP and UI servers
 * 
 * Usage:
 *   overseer-host mcp --cli-path /path/to/os --cwd /path/to/repo --db-path /path/to/tasks.db
 *   overseer-host ui --cli-path /path/to/os --cwd /path/to/repo --db-path /path/to/tasks.db --static-root /path/to/dist --port 6969
 */
import { configureCli } from "./cli.js";
import { startMcpServer } from "./mcp.js";
import { startUiServer } from "./ui.js";

interface Args {
  mode: "mcp" | "ui";
  cliPath: string;
  cwd: string;
  dbPath: string;
  // UI-specific
  staticRoot?: string;
  port?: number;
}

function parseArgs(argv: string[]): Args {
  const args = argv.slice(2); // Skip node and script
  
  if (args.length === 0) {
    printUsage();
    process.exit(1);
  }

  const mode = args[0];
  if (mode !== "mcp" && mode !== "ui") {
    console.error(`Unknown mode: ${mode}`);
    printUsage();
    process.exit(1);
  }

  const result: Args = {
    mode,
    cliPath: "os",
    cwd: process.cwd(),
    dbPath: ".overseer/tasks.db",
  };

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    const next = args[i + 1];

    switch (arg) {
      case "--cli-path":
        if (!next) {
          console.error("--cli-path requires a value");
          process.exit(1);
        }
        result.cliPath = next;
        i++;
        break;
      case "--cwd":
        if (!next) {
          console.error("--cwd requires a value");
          process.exit(1);
        }
        result.cwd = next;
        i++;
        break;
      case "--db-path":
        if (!next) {
          console.error("--db-path requires a value");
          process.exit(1);
        }
        result.dbPath = next;
        i++;
        break;
      case "--static-root":
        if (!next) {
          console.error("--static-root requires a value");
          process.exit(1);
        }
        result.staticRoot = next;
        i++;
        break;
      case "--port":
        if (!next) {
          console.error("--port requires a value");
          process.exit(1);
        }
        result.port = parseInt(next, 10);
        if (isNaN(result.port)) {
          console.error(`Invalid port: ${next}`);
          process.exit(1);
        }
        i++;
        break;
      case "--help":
      case "-h":
        printUsage();
        process.exit(0);
        break;
      default:
        console.error(`Unknown argument: ${arg}`);
        printUsage();
        process.exit(1);
    }
  }

  // Validate UI-specific args
  if (mode === "ui") {
    if (!result.staticRoot) {
      console.error("UI mode requires --static-root");
      process.exit(1);
    }
    if (!result.port) {
      result.port = 6969;
    }
  }

  return result;
}

function printUsage(): void {
  console.log(`
Overseer Host - Unified MCP and UI server

Usage:
  overseer-host <mode> [options]

Modes:
  mcp    Start MCP server (stdio)
  ui     Start UI server (HTTP)

Options:
  --cli-path <path>      Path to os binary (default: "os" in PATH)
  --cwd <path>           Working directory for CLI commands (default: current dir)
  --db-path <path>       Database path passed to os --db (default: .overseer/tasks.db)

UI-specific options:
  --static-root <path>   Path to static files (required for UI mode)
  --port <number>        HTTP port (default: 6969)

Examples:
  overseer-host mcp --cli-path /usr/local/bin/os --cwd /home/user/project --db-path /home/user/project/.overseer/tasks.db
  overseer-host ui --cli-path ./os --cwd . --db-path ./.overseer/tasks.db --static-root ./dist --port 8080
`.trim());
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv);

  // Configure CLI bridge
  configureCli({
    cliPath: args.cliPath,
    cwd: args.cwd,
    dbPath: args.dbPath,
  });

  if (args.mode === "mcp") {
    await startMcpServer();
  } else {
    await startUiServer({
      port: args.port ?? 6969,
      staticRoot: args.staticRoot ?? "./dist",
    });
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
