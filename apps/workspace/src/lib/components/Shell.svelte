<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { Button } from '@intent-studio/ui/ui/button';
	import * as NativeSelect from '@intent-studio/ui/ui/native-select';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { setTheme, theme, type Theme } from '#lib/theme.svelte.ts';

	let { children }: { children: Snippet } = $props();

	const links = [
		{ href: resolve('/'), label: 'Leads', match: (p: string) => p === resolve('/') || p === `${resolve('/')}/` },
		{ href: resolve('/facts'), label: 'Facts', match: (p: string) => p.startsWith(resolve('/facts')) },
		{ href: resolve('/briefs'), label: 'Briefs', match: (p: string) => p.startsWith(resolve('/briefs')) },
		{ href: resolve('/assets'), label: 'Assets', match: (p: string) => p.startsWith(resolve('/assets')) }
	];

	let signingOut = $state(false);
	let signOutFailed = $state(false);

	async function signOut() {
		signingOut = true;
		signOutFailed = false;
		try {
			await api.signOut();
			session.status = 'signed-out';
		} catch {
			// A 503 means the delete was never confirmed: the session is still live, the
			// cookie still held — say the sign-out failed and let the operator retry.
			signOutFailed = true;
		} finally {
			signingOut = false;
		}
	}
</script>

<a
	href="#main"
	class="sr-only z-50 rounded-full bg-primary px-4 py-2 text-sm font-medium text-primary-foreground focus:not-sr-only focus:fixed focus:top-3 focus:left-3"
	>Skip to content</a
>
<header class="border-b border-border">
	<div class="mx-auto flex max-w-6xl flex-wrap items-center gap-x-6 gap-y-3 px-4 py-3 sm:px-6">
		<p class="text-sm font-semibold tracking-tight">Intent Studio</p>
		<nav aria-label="Workspace" class="order-last w-full overflow-x-auto sm:order-none sm:w-auto">
			<ul class="flex gap-1">
				{#each links as link (link.href)}
					{@const current = link.match(page.url.pathname)}
					<li>
						<a
							href={link.href}
							aria-current={current ? 'page' : undefined}
							class="inline-flex h-9 items-center rounded-full px-3.5 text-sm font-medium outline-none transition-colors hover:bg-accent focus-visible:ring-4 focus-visible:ring-ring/25 aria-[current=page]:bg-secondary aria-[current=page]:text-secondary-foreground"
							>{link.label}</a
						>
					</li>
				{/each}
			</ul>
		</nav>
		<div class="ml-auto flex items-center gap-2">
			<label for="theme" class="sr-only">Colour scheme</label>
			<NativeSelect.Root
				id="theme"
				size="sm"
				value={theme.value}
				onchange={(event) => setTheme((event.currentTarget as HTMLSelectElement).value as Theme)}
			>
				<NativeSelect.Option value="system">System</NativeSelect.Option>
				<NativeSelect.Option value="light">Light</NativeSelect.Option>
				<NativeSelect.Option value="dark">Dark</NativeSelect.Option>
			</NativeSelect.Root>
			<Button variant="ghost" size="sm" disabled={signingOut} onclick={signOut}
				>{#if signOutFailed}Sign-out failed — retry{:else}Sign out{/if}</Button
			>
		</div>
	</div>
</header>
<main id="main" tabindex="-1" class="mx-auto max-w-6xl px-4 py-6 outline-none sm:px-6 sm:py-8">
	{@render children()}
</main>
