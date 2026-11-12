// @vitest-environment happy-dom
// MCP servers in the settings (McpServers.svelte, shown in Extensions), mounted in happy-dom with the
// IPC functions replaced. The person's servers list with their switch and their remove button; a
// server the open project declares waits for « Autoriser » / « Refuser » and runs only once allowed;
// `/mcp` opens the settings on the extensions section.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import McpServers from '../../src/lib/features/settings/McpServers.svelte';
import * as ipc from '../../src/lib/ipc/client';
import type { McpServerItem, McpServersView } from '../../src/lib/ipc/client';
import { runSlashLine, type SlashHost } from '../../src/lib/features/commands/dispatch';
import type { SlashAction } from '../../src/lib/ipc/bindings/SlashAction';
import { app } from '../../src/lib/state/app.svelte';
import type { Project } from '../../src/lib/ipc/client';

vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: vi.fn() };
});
vi.mock('$lib/ipc/client', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/ipc/client')>()),
	mcpServers: vi.fn(),
	mcpAdd: vi.fn(),
	mcpRemove: vi.fn(),
	mcpSetEnabled: vi.fn(),
	mcpDecide: vi.fn(),
	mcpTest: vi.fn()
}));

function server(over: Partial<McpServerItem> = {}): McpServerItem {
	return {
		name: 'files',
		source: 'user',
		transport: 'stdio',
		target: 'python3 server.py',
		enabled: true,
		decision: null,
		active: true,
		shadowed: false,
		...over
	};
}

function view(servers: McpServerItem[], project: string | null = null, errors: string[] = []): McpServersView {
	return { project, servers, errors };
}

const PROJECT: Project = {
	id: 'p-demo',
	name: 'Demo',
	path: '/work/demo',
	added_at: 0,
	last_opened_at: 0,
	missing: false,
	chats: false
};

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;

// Lets the pending IPC promises and the component's updates settle.
async function settle() {
	for (let i = 0; i < 3; i++) {
		await new Promise((resolve) => setTimeout(resolve, 0));
		flushSync();
	}
}

async function open(first: McpServersView) {
	vi.mocked(ipc.mcpServers).mockResolvedValue(first);
	component = mount(McpServers, { target });
	await settle();
}

const byLabel = (label: string) => target.querySelector<HTMLElement>(`[aria-label="${label}"]`);

beforeEach(() => {
	target = document.createElement('div');
	document.body.appendChild(target);
	app.projects = [];
	app.currentProjectId = null;
	vi.mocked(ipc.mcpServers).mockReset();
	vi.mocked(ipc.mcpSetEnabled).mockReset();
	vi.mocked(ipc.mcpRemove).mockReset();
	vi.mocked(ipc.mcpDecide).mockReset();
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	target.remove();
});

describe('the person’s servers', () => {
	it('lists each server with its switch and its remove button, and shows the switch state', async () => {
		await open(
			view([
				server({ name: 'files' }),
				server({ name: 'docs', transport: 'http', target: 'https://mcp.example.test/mcp', enabled: false, active: false })
			])
		);

		const files = target.querySelector<HTMLElement>('[data-testid="mcp-server-files"]');
		const docs = target.querySelector<HTMLElement>('[data-testid="mcp-server-docs"]');
		expect(files?.textContent).toContain('python3 server.py');
		expect(docs?.textContent).toContain('https://mcp.example.test/mcp');
		expect(byLabel('Activer files')?.getAttribute('aria-checked')).toBe('true');
		expect(byLabel('Activer docs')?.getAttribute('aria-checked')).toBe('false');
		expect(byLabel('Retirer files')).not.toBeNull();
		expect(byLabel('Retirer docs')).not.toBeNull();
		expect(ipc.mcpServers).toHaveBeenCalledWith(null);
	});

	it('turns a server off and on through the switch, and reads the list it gets back', async () => {
		await open(view([server({ name: 'docs', transport: 'http', enabled: false, active: false })]));
		vi.mocked(ipc.mcpSetEnabled).mockResolvedValue(view([server({ name: 'docs', transport: 'http' })]));

		byLabel('Activer docs')?.click();
		await settle();

		expect(ipc.mcpSetEnabled).toHaveBeenCalledWith('docs', true, null);
		expect(byLabel('Activer docs')?.getAttribute('aria-checked')).toBe('true');
	});

	it('removes a server with its remove button', async () => {
		await open(view([server({ name: 'files' })]));
		vi.mocked(ipc.mcpRemove).mockResolvedValue(view([]));

		byLabel('Retirer files')?.click();
		await settle();

		expect(ipc.mcpRemove).toHaveBeenCalledWith('files', null);
		expect(target.querySelector('[data-testid="mcp-server-files"]')).toBeNull();
		expect(target.textContent).toContain('Aucun serveur');
	});

	it('shows a refusal of the backend as an alert', async () => {
		await open(view([server({ name: 'files' })], null, ['/home/x/.bricks/mcp.toml: unreadable']));
		expect(target.querySelector('[role="alert"]')?.textContent).toContain('unreadable');
	});
});

describe('the servers a project declares', () => {
	beforeEach(() => {
		app.projects = [PROJECT];
		app.currentProjectId = PROJECT.id;
	});

	it('waits for « Autoriser » or « Refuser », and runs nothing before the person decides', async () => {
		const pending = server({ name: 'repo', source: 'project', target: 'node server.js', decision: 'pending', active: false });
		await open(view([pending], PROJECT.path));

		const row = target.querySelector<HTMLElement>('[data-testid="mcp-server-repo"]');
		expect(row?.textContent).toContain("En attente d'autorisation");
		expect(row?.textContent).toContain('Autoriser');
		expect(row?.textContent).toContain('Refuser');
		expect(byLabel('Activer repo')).toBeNull();
		expect(byLabel('Tester repo')).toBeNull();
		expect(ipc.mcpServers).toHaveBeenCalledWith(PROJECT.id);

		vi.mocked(ipc.mcpDecide).mockResolvedValue(
			view([server({ name: 'repo', source: 'project', target: 'node server.js', decision: 'approved', active: true })], PROJECT.path)
		);
		const approve = [...row!.querySelectorAll('button')].find((b) => b.textContent?.includes('Autoriser'));
		approve?.click();
		await settle();

		expect(ipc.mcpDecide).toHaveBeenCalledWith(PROJECT.id, 'repo', true);
		const after = target.querySelector<HTMLElement>('[data-testid="mcp-server-repo"]');
		expect(after?.textContent).not.toContain('Autoriser');
		expect(after?.textContent).toContain('Refuser');
	});

	it('shows a refused server as refused, and offers to allow it again', async () => {
		await open(
			view([server({ name: 'repo', source: 'project', decision: 'denied', active: false })], PROJECT.path)
		);
		const row = target.querySelector<HTMLElement>('[data-testid="mcp-server-repo"]');
		expect(row?.textContent).toContain('Refusé');
		expect(row?.textContent).toContain('Autoriser');
		expect(row?.textContent).not.toContain('Refuser');
	});
});

describe('/mcp', () => {
	it('opens the settings on the extensions section', async () => {
		const action: SlashAction = { kind: 'present', present: { view: 'mcp' } };
		const openSettings = vi.fn();
		const open = vi.fn();
		const host = {
			parse: async () => action,
			openSettings,
			open
		} as unknown as SlashHost;

		await runSlashLine('/mcp', host);

		expect(openSettings).toHaveBeenCalledWith('extensions');
		expect(open).not.toHaveBeenCalledWith('mcp', expect.anything());
		expect(open).not.toHaveBeenCalledWith('mcp', undefined);
	});
});
