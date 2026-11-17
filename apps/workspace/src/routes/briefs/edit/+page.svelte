<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Alert from '@intent-studio/ui/ui/alert';
	import { Button } from '@intent-studio/ui/ui/button';
	import BriefForm from '#lib/components/BriefForm.svelte';
	import ClientBar from '#lib/components/ClientBar.svelte';
	import ListSkeleton from '#lib/components/ListSkeleton.svelte';
	import { ApiError, api, type Asset, type BriefRecord, type Fact, type Problem } from '#lib/api.ts';
	import { clients, loadClients } from '#lib/clients.svelte.ts';
	import { sessionEnded } from '#lib/session.svelte.ts';
	import { failureText } from '#lib/words.ts';

	const id = $derived(page.url.searchParams.get('id'));
	let status = $state<'loading' | 'ready' | 'error'>('loading');
	let loadError = $state('');
	let record = $state<BriefRecord | null>(null);
	let facts = $state<Fact[]>([]);
	let assets = $state<Asset[]>([]);
	let problems = $state<Problem[]>([]);
	let saveError = $state<string | null>(null);
	let conflict = $state<number | null>(null);
	let saving = $state(false);

	async function load(tenant: string, brief: string | null) {
		status = 'loading';
		try {
			const [factList, assetList, existing] = await Promise.all([
				api.facts(tenant),
				api.assets(tenant),
				brief ? api.brief(tenant, brief) : Promise.resolve(null)
			]);
			facts = factList.facts;
			assets = assetList.assets;
			record = existing;
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
		if (clients.selected) void load(clients.selected, id);
	});

	async function save(brief: unknown) {
		saving = true;
		problems = [];
		saveError = null;
		conflict = null;
		try {
			const saved = record
				? await api.reviseBrief(clients.selected, record.id, record.revision, brief)
				: await api.createBrief(clients.selected, brief);
			await goto(`${resolve('/briefs/view')}?id=${saved.id}&saved=${saved.revision}`);
		} catch (error) {
			if (sessionEnded(error)) return;
			if (error instanceof ApiError && error.code === 'revision_conflict') {
				conflict = error.problems[0]?.latest_revision ?? null;
			} else if (error instanceof ApiError && error.problems.length > 0) {
				// A revision's problems are reported under `brief.`; the form's paths are the brief's own.
				problems = error.problems.map((p) => ({ ...p, path: p.path.replace(/^brief\.?/, '') }));
				window.scrollTo({ top: 0 });
			} else {
				saveError = failureText(error, true);
			}
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>{id ? 'Edit brief' : 'New brief'} · Intent Studio</title></svelte:head>

<div class="mb-4">
	<p class="text-sm"><a class="underline underline-offset-4" href={resolve('/briefs')}>Briefs</a></p>
	<h1 class="mt-1 text-2xl font-semibold tracking-tight">
		{#if record}Edit the brief for {record.brief.business_name}{:else}New brief{/if}
	</h1>
	{#if record}<p class="mt-1 text-sm text-muted-foreground">Saving makes revision {record.revision + 1}; revision {record.revision} stays as it was.</p>{/if}
</div>

{#if !id}<ClientBar />{/if}

{#if conflict !== null}
	<Alert.Root role="alert" class="mb-6">
		<Alert.Title>Someone saved a newer revision</Alert.Title>
		<Alert.Description>
			<p>Revision {conflict} was saved after you opened this one, so your changes were not saved. They are still in the form: copy what you need, then reload the newest revision.</p>
			<Button class="mt-3" variant="outline" size="sm" onclick={() => load(clients.selected, id)}>Reload the newest revision</Button>
		</Alert.Description>
	</Alert.Root>
{/if}
{#if saveError}
	<Alert.Root role="alert" class="mb-6"><Alert.Title>The brief may not be saved</Alert.Title><Alert.Description>{saveError}</Alert.Description></Alert.Root>
{/if}

{#if clients.status === 'ready' && clients.selected}
	{#if status === 'loading'}
		<ListSkeleton label="Loading the brief" rows={3} />
	{:else if status === 'error'}
		<Alert.Root role="alert">
			<Alert.Title>Could not load what the brief needs</Alert.Title>
			<Alert.Description>
				<p>{loadError}</p>
				<Button class="mt-3" variant="outline" size="sm" onclick={() => load(clients.selected, id)}>Try again</Button>
			</Alert.Description>
		</Alert.Root>
	{:else}
		{#key record?.revision ?? 0}
			<BriefForm
				initial={record?.brief ?? null}
				{facts}
				{assets}
				{problems}
				{saving}
				submitLabel={record ? `Save revision ${record.revision + 1}` : 'Save the brief'}
				onsave={save}
			/>
		{/key}
	{/if}
{/if}
