<script lang="ts">
	import '../app.css';
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { Button } from '@intent-studio/ui/ui/button';
	import { Skeleton } from '@intent-studio/ui/ui/skeleton';
	import Shell from '#lib/components/Shell.svelte';
	import SignIn from '#lib/components/SignIn.svelte';
	import { checkSession, session } from '#lib/session.svelte.ts';
	import { loadTheme } from '#lib/theme.svelte.ts';

	let { children }: { children: Snippet } = $props();

	onMount(() => {
		loadTheme();
		void checkSession();
	});
</script>

<svelte:head>
	<title>Intent Studio</title>
</svelte:head>

<div class="min-h-dvh bg-background text-foreground">
	{#if session.status === 'signed-in'}
		<Shell>{@render children()}</Shell>
	{:else}
		<header class="border-b border-border">
			<div class="mx-auto max-w-6xl px-4 py-3 sm:px-6">
				<p class="text-sm font-semibold tracking-tight">Intent Studio</p>
			</div>
		</header>
		<main id="main" class="mx-auto max-w-6xl px-4 py-10 sm:px-6">
			{#if session.status === 'checking'}
				<div class="mx-auto grid max-w-sm gap-3" aria-busy="true" aria-label="Checking your session">
					<Skeleton class="h-6 w-40" />
					<Skeleton class="h-11 w-full rounded-lg" />
					<Skeleton class="h-10 w-full rounded-full" />
				</div>
			{:else if session.status === 'unreachable'}
				<div role="alert" class="mx-auto grid max-w-sm gap-4 rounded-xl border border-border bg-card p-6">
					<h2 class="text-lg font-semibold">The server cannot be reached</h2>
					<p class="text-sm text-muted-foreground">Leads already saved are safe. Check your connection, then try again.</p>
					<Button onclick={() => checkSession()}>Try again</Button>
				</div>
			{:else}
				<SignIn onsignedin={() => (session.status = 'signed-in')} />
			{/if}
		</main>
	{/if}
</div>
