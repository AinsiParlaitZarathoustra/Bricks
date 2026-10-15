// Test fixtures for the WebDriver run: a throwaway Bricks home with one registered project.
// Nothing here is product data. The layout follows crates/bricks-config (BRICKS_HOME) and
// src-tauri/src/services/projects.rs (desktop/projects.json, schema 1).
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const DEMO_PROJECT_ID = 'e2e-demo-project';
export const DEMO_PROJECT_NAME = 'Projet de démonstration';

export interface SeededHome {
	/** Value for BRICKS_HOME. */
	home: string;
	/** Canonical path of the registered project folder. */
	projectDir: string;
}

export function seedBricksHome(): SeededHome {
	const root = realpathSync(mkdtempSync(path.join(os.tmpdir(), 'bricks-e2e-')));
	const home = path.join(root, 'home');
	const projectDir = path.join(root, 'demo-project');

	mkdirSync(path.join(home, 'desktop'), { recursive: true });
	mkdirSync(projectDir, { recursive: true });
	writeFileSync(path.join(projectDir, 'README.md'), '# Projet de démonstration\n');
	writeFileSync(
		path.join(home, 'desktop', 'projects.json'),
		JSON.stringify(
			{
				schema: 1,
				projects: [
					{
						id: DEMO_PROJECT_ID,
						name: DEMO_PROJECT_NAME,
						path: projectDir,
						added_at: 0,
						last_opened_at: 0
					}
				]
			},
			null,
			2
		)
	);
	// No providers.toml on purpose: the start path must also work (and show the
	// "Aucun fournisseur configuré" state). Add one only for a run that sends prompts.
	return { home, projectDir };
}
