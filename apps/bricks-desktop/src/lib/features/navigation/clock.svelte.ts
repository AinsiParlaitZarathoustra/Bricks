// A reactive "now" that refreshes every minute, so relative dates stay current.
// Call it while a component initialises: it creates an $effect for the timer.

export function createClock(intervalMs = 60_000) {
	let now = $state(Date.now());

	$effect(() => {
		const id = setInterval(() => {
			now = Date.now();
		}, intervalMs);
		return () => clearInterval(id);
	});

	return {
		get now() {
			return now;
		}
	};
}
