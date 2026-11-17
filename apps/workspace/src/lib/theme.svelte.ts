// Light, dark, or whatever the system prefers. The kit's dark tokens apply under `.dark` on the
// root element; the choice is a per-viewer convenience kept in localStorage, and the page works
// the same without it.

export type Theme = 'system' | 'light' | 'dark';

const KEY = 'intent-studio-theme';

export const theme = $state<{ value: Theme }>({ value: 'system' });

function prefersDark(): boolean {
	return typeof matchMedia === 'function' && matchMedia('(prefers-color-scheme: dark)').matches;
}

function apply(): void {
	const dark = theme.value === 'dark' || (theme.value === 'system' && prefersDark());
	document.documentElement.classList.toggle('dark', dark);
	document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
}

export function loadTheme(): void {
	try {
		const saved = localStorage.getItem(KEY);
		if (saved === 'light' || saved === 'dark' || saved === 'system') theme.value = saved;
	} catch {
		// Storage can be unavailable (private windows, blocked site data): the system preference holds.
	}
	apply();
	if (typeof matchMedia === 'function') {
		matchMedia('(prefers-color-scheme: dark)').addEventListener('change', apply);
	}
}

export function setTheme(value: Theme): void {
	theme.value = value;
	try {
		localStorage.setItem(KEY, value);
	} catch {
		// See loadTheme.
	}
	apply();
}
