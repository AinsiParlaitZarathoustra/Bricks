// WebDriver UI tests for Bricks Desktop: WebdriverIO + tauri-driver.
//
// Supported platforms: Linux and Windows only. Tauri has no WebDriver for macOS (WKWebView),
// so on macOS the manual checklist in tests/README.md applies instead.
//
// Not runnable until the dev dependencies listed in tests/README.md are added to package.json
// (orchestrator). Also needs `tauri-driver` (cargo install tauri-driver) and, on Linux,
// WebKitWebDriver (package webkit2gtk-driver); on Windows, msedgedriver matching the WebView2 runtime.
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { seedBricksHome } from './seed';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '../..'); // apps/bricks-desktop
const repoRoot = path.resolve(appRoot, '../..'); // Cargo workspace root (shared target/)

const isWindows = process.platform === 'win32';
const binary =
	process.env.BRICKS_BINARY ??
	path.join(repoRoot, 'target', 'debug', isWindows ? 'bricks-desktop.exe' : 'bricks-desktop');
const tauriDriver = path.join(os.homedir(), '.cargo', 'bin', isWindows ? 'tauri-driver.exe' : 'tauri-driver');

let driver: ChildProcess | undefined;

export const config = {
	hostname: '127.0.0.1',
	port: 4444,
	specs: [path.join(here, 'specs', '**', '*.e2e.ts')],
	maxInstances: 1,
	capabilities: [
		{
			maxInstances: 1,
			'tauri:options': { application: binary }
		}
	],
	framework: 'mocha',
	reporters: ['spec'],
	mochaOpts: { ui: 'bdd', timeout: 60_000 },
	logLevel: 'warn',

	onPrepare() {
		// The app reads its home from BRICKS_HOME; tauri-driver inherits this environment.
		const seeded = seedBricksHome();
		process.env.BRICKS_HOME = seeded.home;
		process.env.BRICKS_E2E_PROJECT_DIR = seeded.projectDir;
		// Build the debug binary (skip with BRICKS_BINARY pointing at an existing build).
		if (!process.env.BRICKS_BINARY) {
			const build = spawnSync('cargo', ['build', '-p', 'bricks-desktop'], {
				cwd: repoRoot,
				stdio: 'inherit',
				shell: isWindows
			});
			if (build.status !== 0) throw new Error('cargo build -p bricks-desktop failed');
		}
	},

	// Start tauri-driver before each session so WebDriver requests are proxied to the app.
	beforeSession() {
		driver = spawn(tauriDriver, [], { stdio: [null, process.stdout, process.stderr] });
	},

	afterSession() {
		driver?.kill();
		driver = undefined;
	}
};
