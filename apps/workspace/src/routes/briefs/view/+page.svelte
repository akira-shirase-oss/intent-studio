<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Alert from '@intent-studio/ui/ui/alert';
	import { Badge } from '@intent-studio/ui/ui/badge';
	import { Button } from '@intent-studio/ui/ui/button';
	import { Label } from '@intent-studio/ui/ui/label';
	import * as NativeSelect from '@intent-studio/ui/ui/native-select';
	import ListSkeleton from '#lib/components/ListSkeleton.svelte';
	import { ApiError, api, type Asset, type BriefRecord, type Fact, type Snapshot } from '#lib/api.ts';
	import { clients, loadClients } from '#lib/clients.svelte.ts';
	import { sessionEnded } from '#lib/session.svelte.ts';
	import { failureText, factStatus, kindLabels, permissionLabels, refusalText, when } from '#lib/words.ts';

	const id = $derived(page.url.searchParams.get('id') ?? '');
	const saved = $derived(page.url.searchParams.get('saved'));
	const wanted = $derived(Number(page.url.searchParams.get('revision')) || undefined);

	let status = $state<'loading' | 'ready' | 'error'>('loading');
	let loadError = $state('');
	let record = $state<BriefRecord | null>(null);
	let facts = $state<Fact[]>([]);
	let assets = $state<Asset[]>([]);
	let snapshots = $state<Snapshot[]>([]);
	let storeOn = $state(true);
	let fetching = $state<number | null>(null);
	let fetchResult = $state<{ index: number; ok: boolean; text: string } | null>(null);

	async function load(tenant: string, brief: string, revision: number | undefined) {
		status = 'loading';
		try {
			const [briefRecord, factList, assetList, snapshotList] = await Promise.all([
				api.brief(tenant, brief, revision),
				api.facts(tenant),
				api.assets(tenant),
				api.snapshots(tenant, brief)
			]);
			record = briefRecord;
			facts = factList.facts;
			assets = assetList.assets;
			snapshots = snapshotList.snapshots;
			storeOn = snapshotList.store;
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
		if (clients.selected && id) void load(clients.selected, id, wanted);
	});

	const factById = $derived(new Map(facts.map((f) => [f.id, f])));
	const logo = $derived(record?.brief.brand.logo ? assets.find((a) => a.id === record?.brief.brand.logo) : undefined);
	const latestFor = (url: string) => snapshots.find((s) => s.url === url);

	async function fetchCopy(index: number) {
		if (!record) return;
		fetching = index;
		fetchResult = null;
		try {
			const snapshot = await api.fetchSnapshot(clients.selected, record.id, index);
			snapshots = [snapshot, ...snapshots];
			fetchResult = { index, ok: true, text: `Saved a copy (${Math.ceil((snapshot.bytes ?? 0) / 1024)} KiB).` };
		} catch (error) {
			if (sessionEnded(error)) return;
			const reason = error instanceof ApiError && error.code === 'fetch_refused' ? error.problems[0]?.code : undefined;
			fetchResult = { index, ok: false, text: reason ? `Not fetched. ${refusalText(reason)}` : failureText(error, true) };
			// A refusal is recorded too: show it in the history.
			try {
				snapshots = (await api.snapshots(clients.selected, record.id)).snapshots;
			} catch {
				// The history refreshes on the next visit.
			}
		} finally {
			fetching = null;
		}
	}
</script>

<svelte:head><title>{record ? `${record.brief.business_name} brief` : 'Brief'} · Intent Studio</title></svelte:head>

<p class="mb-1 text-sm"><a class="underline underline-offset-4" href={resolve('/briefs')}>Briefs</a></p>

{#if status === 'loading'}
	<ListSkeleton label="Loading the brief" rows={3} />
{:else if status === 'error'}
	<Alert.Root role="alert">
		<Alert.Title>Could not load the brief</Alert.Title>
		<Alert.Description>
			<p>{loadError}</p>
			<Button class="mt-3" variant="outline" size="sm" onclick={() => load(clients.selected, id, wanted)}>Try again</Button>
		</Alert.Description>
	</Alert.Root>
{:else if record}
	{@const brief = record.brief}
	<div class="mb-6 flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">{brief.business_name}</h1>
			<p class="mt-1 text-sm text-muted-foreground">Revision {record.revision} of {record.latest_revision}, saved {when(record.revised_at)}</p>
		</div>
		<div class="flex flex-wrap items-end gap-3">
			{#if record.latest_revision > 1}
				<div class="grid gap-2">
					<Label for="revision">Revision</Label>
					<NativeSelect.Root
						id="revision"
						value={String(record.revision)}
						onchange={(event) => goto(`${resolve('/briefs/view')}?id=${id}&revision=${(event.currentTarget as HTMLSelectElement).value}`)}
					>
						{#each Array.from({ length: record.latest_revision }, (_, i) => record!.latest_revision - i) as n (n)}
							<NativeSelect.Option value={String(n)}>Revision {n}{n === record.latest_revision ? ' (newest)' : ''}</NativeSelect.Option>
						{/each}
					</NativeSelect.Root>
				</div>
			{/if}
			{#if record.revision === record.latest_revision}
				<Button href={`${resolve('/briefs/edit')}?id=${id}`}>Edit</Button>
			{/if}
		</div>
	</div>

	{#if saved}
		<p role="status" class="mb-6 rounded-xl border border-border bg-live-soft px-4 py-3 text-sm text-live-ink">Saved as revision {saved}.</p>
	{/if}

	<div class="grid gap-6 lg:grid-cols-2">
		<section aria-labelledby="v-intent" class="rounded-xl border border-border bg-card p-4 sm:p-6">
			<h2 id="v-intent" class="text-lg font-semibold">What the client wants</h2>
			<p class="mt-2 whitespace-pre-line">{brief.intent}</p>
			<h3 class="mt-4 text-sm font-medium text-muted-foreground">Who it is for</h3>
			<p class="mt-1 whitespace-pre-line">{brief.audience}</p>
			<h3 class="mt-4 text-sm font-medium text-muted-foreground">Goal</h3>
			<p class="mt-1">Quote requests</p>
		</section>
		<section aria-labelledby="v-services" class="rounded-xl border border-border bg-card p-4 sm:p-6">
			<h2 id="v-services" class="text-lg font-semibold">Services</h2>
			<ul class="mt-2 grid gap-2">
				{#each brief.services as service (service.name)}
					<li><span class="font-medium">{service.name}</span>{#if service.description}<span class="text-muted-foreground">{` — ${service.description}`}</span>{/if}</li>
				{/each}
			</ul>
		</section>
		<section aria-labelledby="v-brand" class="rounded-xl border border-border bg-card p-4 sm:p-6">
			<h2 id="v-brand" class="text-lg font-semibold">Look and tone</h2>
			<p class="mt-2">{brief.brand.tone ?? 'No tone noted.'}</p>
			{#if brief.brand.colors.length > 0}
				<ul class="mt-3 flex flex-wrap gap-3" aria-label="Brand colours">
					{#each brief.brand.colors as color (color)}
						<li class="flex items-center gap-2 text-sm"><span class="size-6 rounded-md border border-border" style={`background:${color}`} aria-hidden="true"></span><span class="font-mono">{color}</span></li>
					{/each}
				</ul>
			{/if}
			{#if logo}
				<div class="mt-4 flex items-center gap-3">
					<img src={api.assetUrl(clients.selected, logo.id)} alt="The client’s logo" class="h-12 w-auto max-w-40 rounded border border-border bg-white object-contain p-1" />
					<span class="text-sm text-muted-foreground">{logo.original.name ?? 'Logo'}</span>
				</div>
			{:else if brief.brand.logo}
				<p class="mt-3 text-sm text-muted-foreground">The logo file is no longer listed.</p>
			{/if}
			{#if brief.domains.length > 0}
				<h3 class="mt-4 text-sm font-medium text-muted-foreground">Own web addresses</h3>
				<p class="mt-1">{brief.domains.join(', ')}</p>
			{/if}
		</section>
		<section aria-labelledby="v-claims" class="rounded-xl border border-border bg-card p-4 sm:p-6">
			<h2 id="v-claims" class="text-lg font-semibold">Approved claims</h2>
			{#if brief.claims.length === 0}
				<p class="mt-2 text-sm text-muted-foreground">None yet: pages built from this brief can make no factual claim.</p>
			{:else}
				<ul class="mt-2 grid gap-2">
					{#each brief.claims as claim (claim)}
						{@const fact = factById.get(claim)}
						<li class="rounded-lg border border-border p-3">
							{#if fact}
								{@const label = factStatus(fact)}
								<div class="flex flex-wrap items-center justify-between gap-2">
									<span class="text-xs font-medium text-muted-foreground">{kindLabels[fact.kind]}</span>
									<Badge variant={label.variant}>{label.text}</Badge>
								</div>
								<p class="mt-1">{fact.statement}</p>
							{:else}
								<p class="text-sm text-muted-foreground">A fact that is no longer listed.</p>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	</div>

	<section aria-labelledby="v-references" class="mt-6 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="v-references" class="text-lg font-semibold">Reference sites</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			A copy is fetched only from a public web address: private, local and reserved addresses are never fetched. The copy is kept as it is, not read.
		</p>
		{#if !storeOn}
			<p class="mt-3 text-sm text-attention-ink">This server has no file store configured, so copies cannot be kept.</p>
		{/if}
		{#if brief.references.length === 0}
			<p class="mt-3 text-sm text-muted-foreground">No reference sites in this revision.</p>
		{:else}
			<ul class="mt-3 grid gap-3">
				{#each brief.references as reference, i (reference.url)}
					{@const last = latestFor(reference.url)}
					<li class="rounded-lg border border-border p-4">
						<p class="font-medium break-all">{reference.url}</p>
						<p class="mt-1 text-sm text-muted-foreground">{permissionLabels[reference.permission]}</p>
						{#if reference.notes}<p class="mt-1 text-sm whitespace-pre-line">{reference.notes}</p>{/if}
						<div class="mt-3 flex flex-wrap items-center gap-3">
							{#if record.revision === record.latest_revision}
								<Button size="sm" variant="outline" disabled={fetching !== null || !storeOn} onclick={() => fetchCopy(i)}>
									{fetching === i ? 'Fetching…' : 'Fetch a copy'}
								</Button>
							{/if}
							{#if fetchResult?.index === i}
								<p role="status" class={`text-sm ${fetchResult.ok ? 'text-live-ink' : 'text-attention-ink'}`}>{fetchResult.text}</p>
							{:else if last}
								<p class="text-sm text-muted-foreground">
									{last.outcome === 'stored' ? `Copy saved ${when(last.fetched_at)}` : `Last try ${when(last.fetched_at)} was refused: ${refusalText(last.refusal)}`}
								</p>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
		{#if snapshots.length > 0}
			<details class="mt-4 text-sm">
				<summary class="cursor-pointer rounded font-medium outline-none focus-visible:ring-4 focus-visible:ring-ring/25">Every fetch, newest first ({snapshots.length})</summary>
				<ul class="mt-2 grid gap-1 text-muted-foreground">
					{#each snapshots as snapshot (snapshot.id)}
						<li class="break-all">
							{when(snapshot.fetched_at)} · {snapshot.url} · {snapshot.outcome === 'stored' ? `saved, ${snapshot.bytes ?? 0} bytes` : `refused (${snapshot.refusal})`}
						</li>
					{/each}
				</ul>
			</details>
		{/if}
	</section>
{/if}
