<script lang="ts">
	import { resolve } from '$app/paths';
	import * as Alert from '@intent-studio/ui/ui/alert';
	import { Badge } from '@intent-studio/ui/ui/badge';
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Empty from '@intent-studio/ui/ui/empty';
	import ClientBar from '#lib/components/ClientBar.svelte';
	import ListSkeleton from '#lib/components/ListSkeleton.svelte';
	import { api, type BriefSummary } from '#lib/api.ts';
	import { clients, loadClients } from '#lib/clients.svelte.ts';
	import { sessionEnded } from '#lib/session.svelte.ts';
	import { failureText, when } from '#lib/words.ts';

	let status = $state<'loading' | 'ready' | 'error'>('loading');
	let briefs = $state<BriefSummary[]>([]);
	let loadError = $state('');
	const client = $derived(clients.list.find((c) => c.id === clients.selected));

	async function load(tenant: string) {
		status = 'loading';
		try {
			briefs = (await api.briefs(tenant)).briefs;
			status = 'ready';
		} catch (error) {
			if (sessionEnded(error)) return;
			loadError = failureText(error);
			status = 'error';
		}
	}

	$effect(() => {
		void loadClients();
	});
	$effect(() => {
		if (clients.selected) void load(clients.selected);
	});
</script>

<svelte:head><title>Briefs · Intent Studio</title></svelte:head>

<div class="mb-4">
	<h1 class="text-2xl font-semibold tracking-tight">Briefs</h1>
	<p class="mt-1 max-w-2xl text-sm text-muted-foreground">
		What the client wants their site to do, who it is for, what they offer, how it should look, which sites they pointed
		at, and the approved facts it may claim. Every save keeps the earlier revisions.
	</p>
</div>

<ClientBar />

{#if clients.status === 'ready' && client}
	<div class="mb-4 flex justify-end">
		<Button href={resolve('/briefs/edit')}>New brief</Button>
	</div>
	{#if status === 'loading'}
		<ListSkeleton label="Loading briefs" rows={2} />
	{:else if status === 'error'}
		<Alert.Root role="alert">
			<Alert.Title>Could not load the briefs</Alert.Title>
			<Alert.Description>
				<p>{loadError}</p>
				<Button class="mt-3" variant="outline" size="sm" onclick={() => load(clients.selected)}>Try again</Button>
			</Alert.Description>
		</Alert.Root>
	{:else if briefs.length === 0}
		<Empty.Root class="border border-dashed border-border">
			<Empty.Header>
				<Empty.Title>No brief yet for {client.name}</Empty.Title>
				<Empty.Description>Write down what the site is for. Record the client’s facts first if you want the brief to list them.</Empty.Description>
			</Empty.Header>
			<Empty.Content class="flex flex-wrap justify-center gap-3">
				<Button href={resolve('/briefs/edit')}>Write the brief</Button>
				<Button variant="outline" href={resolve('/facts')}>Record facts first</Button>
			</Empty.Content>
		</Empty.Root>
	{:else}
		<ul class="grid gap-3" aria-label={`Briefs for ${client.name}`}>
			{#each briefs as brief (brief.id)}
				<li class="list-row rounded-xl border border-border bg-card p-4">
					<div class="flex flex-wrap items-start justify-between gap-2">
						<a class="font-medium underline-offset-4 hover:underline focus-visible:underline" href={`${resolve('/briefs/view')}?id=${brief.id}`}>
							{brief.business_name}
						</a>
						<Badge variant="outline">Revision {brief.latest_revision}</Badge>
					</div>
					<p class="mt-1 text-sm text-muted-foreground">Last saved {when(brief.updated_at)}</p>
				</li>
			{/each}
		</ul>
	{/if}
{/if}
