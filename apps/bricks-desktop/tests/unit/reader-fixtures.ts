// Fixtures of the reader tests: a fake disk answered the way Rust answers
// `reader_file_read`, answers the test gives itself, and edit items.

import type { ReaderFileRequest, ReaderFileWindow } from '../../src/lib/ipc/reader';
import type { EditItem } from '../../src/lib/state/conversation.svelte';

/** The project folder and the worktree of the isolated agent `a1`. */
export const PROJECT = '/proj';
export const WORKTREE = '/wt/a1';
export const SESSION = 's1';

/** Window size of Rust (`WINDOW_LINES`). */
const WINDOW = 400;

/** Absolute path → lines of the file. */
export type Disk = Record<string, string[]>;

/** The folder a request resolves in, as Rust resolves it. */
function rootOf(request: ReaderFileRequest): string {
	return request.agentId ? WORKTREE : PROJECT;
}

/** The answer of Rust to `reader_file_read` for `request`, from `disk`. Throws the message Rust gives for a missing file. */
export function answer(disk: Disk, request: ReaderFileRequest): ReaderFileWindow {
	const absolute = request.path.startsWith('/') ? request.path : `${rootOf(request)}/${request.path}`;
	const lines = disk[absolute];
	if (!lines) throw new Error(`\`${request.path}\` n'existe plus : le fichier a été supprimé ou déplacé`);
	const total = lines.length;
	const first = request.line ?? 1;
	const last = Math.min(total, request.endLine ?? first + WINDOW - 1, first + WINDOW - 1);
	return {
		path: request.path,
		start: first,
		lines: lines.slice(first - 1, last),
		totalLines: total,
		truncated: total > WINDOW,
		language: null,
		absolute,
	};
}

/** A reader that answers at once from `disk`, and keeps the requests it got. */
export function diskRead(disk: Disk) {
	const requests: ReaderFileRequest[] = [];
	const read = async (request: ReaderFileRequest): Promise<ReaderFileWindow> => {
		requests.push(request);
		return answer(disk, request);
	};
	return { read, requests };
}

/** A pending read, answered by the test in the order it chooses. */
export interface PendingRead {
	request: ReaderFileRequest;
	resolve: (window: ReaderFileWindow) => void;
	reject: (error: Error) => void;
}

/** A reader whose answers come when the test gives them. */
export function manualRead() {
	const pending: PendingRead[] = [];
	const read = (request: ReaderFileRequest) =>
		new Promise<ReaderFileWindow>((resolve, reject) => {
			pending.push({ request, resolve, reject });
		});
	return { read, pending };
}

/** Lets the pending promises settle. */
export const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** The lines `ligne 1` … `ligne n`. */
export function numbered(n: number): string[] {
	return Array.from({ length: n }, (_, i) => `ligne ${i + 1}`);
}

/** One edited file of an edit item, as the engine records it. */
export interface EditedFixture {
	path: string;
	absolute?: string;
	kind?: 'create' | 'modify' | 'delete';
	added?: number;
	removed?: number;
	patch?: string | null;
	patchTruncated?: boolean;
}

/** An `edit` item of the timeline with the given files. */
export function editItem(id: string, files: EditedFixture[], agentId?: string): EditItem {
	return {
		kind: 'edit',
		id,
		toolCallId: `tc-${id}`,
		tool: 'apply_patch',
		files: files.map((file) => ({
			kind: 'modify',
			added: 1,
			removed: 0,
			binary: false,
			...file,
		})) as unknown as EditItem['files'],
		...(agentId ? { agentId } : {}),
	};
}
