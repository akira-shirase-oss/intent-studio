<script lang="ts">
	import * as Alert from '@intent-studio/ui/ui/alert';
	import { Badge } from '@intent-studio/ui/ui/badge';
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Empty from '@intent-studio/ui/ui/empty';
	import { Input } from '@intent-studio/ui/ui/input';
	import { Label } from '@intent-studio/ui/ui/label';
	import ClientBar from '#lib/components/ClientBar.svelte';
	import ListSkeleton from '#lib/components/ListSkeleton.svelte';
	import { ApiError, api, type Asset } from '#lib/api.ts';
	import { clients, loadClients } from '#lib/clients.svelte.ts';
	import { sessionEnded } from '#lib/session.svelte.ts';
	import { failureText, problemText, refusalText, when } from '#lib/words.ts';

	let status = $state<'loading' | 'ready' | 'error'>('loading');
	let assets = $state<Asset[]>([]);
	let storeOn = $state(true);
	let loadError = $state('');

	let files = $state<FileList | undefined>(undefined);
	let licence = $state('');
	let source = $state('');
	let uploading = $state(false);
	let result = $state<{ ok: boolean; text: string } | null>(null);
	let fieldErrors = $state<Record<string, string>>({});
	let fileInput = $state<HTMLInputElement | null>(null);

	const client = $derived(clients.list.find((c) => c.id === clients.selected));

	async function load(tenant: string) {
		status = 'loading';
		try {
			const list = await api.assets(tenant);
			assets = list.assets;
			storeOn = list.store;
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

	function size(bytes: number): string {
		return bytes < 1024 ? `${bytes} bytes` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KiB` : `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
	}

	function kindOf(asset: Asset): string {
		const kinds: Record<string, string> = {
			'image/svg+xml': 'SVG image',
			'image/png': 'PNG image',
			'image/jpeg': 'JPEG image',
			'font/ttf': 'TrueType font',
			'font/otf': 'OpenType font'
		};
		return kinds[asset.media_type] ?? asset.media_type;
	}

	async function upload(event: SubmitEvent) {
		event.preventDefault();
		const file = files?.[0];
		fieldErrors = {};
		result = null;
		if (!file) {
			fieldErrors = { file: 'Choose a file.' };
			return;
		}
		uploading = true;
		try {
			const asset = await api.uploadAsset(clients.selected, file, licence, source);
			assets = [asset, ...assets];
			result = {
				ok: true,
				text:
					asset.removed_parts > 0
						? `Saved ${asset.original.name ?? 'the file'}. ${asset.removed_parts} unsafe part${asset.removed_parts === 1 ? ' was' : 's were'} removed (scripts, links or embedded content); the original is kept as it was.`
						: `Saved ${asset.original.name ?? 'the file'}.`
			};
			files = undefined;
			if (fileInput) fileInput.value = '';
			licence = '';
			source = '';
		} catch (error) {
			if (sessionEnded(error)) return;
			if (error instanceof ApiError && error.code === 'asset_refused') {
				const problem = error.problems[0];
				if (problem && (problem.path === 'licence' || problem.path === 'source' || problem.path === 'file')) {
					fieldErrors = Object.fromEntries(error.problems.map((p) => [p.path, problemText(p)]));
					result = { ok: false, text: 'The file was not saved. See the fields below.' };
				} else {
					result = { ok: false, text: `The file was not saved. ${refusalText(problem?.code)}` };
				}
			} else {
				result = { ok: false, text: failureText(error, true) };
			}
		} finally {
			uploading = false;
		}
	}
</script>

<svelte:head><title>Assets · Intent Studio</title></svelte:head>

<div class="mb-4">
	<h1 class="text-2xl font-semibold tracking-tight">Assets</h1>
	<p class="mt-1 max-w-2xl text-sm text-muted-foreground">
		Logos, photos and fonts, each kept with its licence and where it came from. Every file is checked in a separate,
		locked-down process before it is kept: SVGs lose anything that could run or load something, images are re-encoded
		without their hidden metadata.
	</p>
</div>

<ClientBar />

{#if clients.status === 'ready' && client}
	<section aria-labelledby="upload-title" class="mb-6 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="upload-title" class="text-lg font-semibold">Upload a file for {client.name}</h2>
		{#if !storeOn}
			<p role="alert" class="mt-2 text-sm text-attention-ink">This server has no file store configured, so files cannot be kept. Whoever runs it needs to set INTENT_STORE_DIR.</p>
		{/if}
		<form class="mt-4 grid gap-4" onsubmit={upload}>
			<div class="grid gap-2">
				<Label for="asset-file">File (SVG, PNG or JPEG image, or a TTF or OTF font; up to 10 MiB)</Label>
				<Input
					id="asset-file"
					type="file"
					bind:ref={fileInput}
					bind:files
					accept=".svg,.png,.jpg,.jpeg,.ttf,.otf,image/svg+xml,image/png,image/jpeg,font/ttf,font/otf"
					aria-invalid={fieldErrors['file'] ? 'true' : undefined}
					aria-describedby={fieldErrors['file'] ? 'asset-file-error' : undefined}
				/>
				{#if fieldErrors['file']}<p id="asset-file-error" class="text-sm text-attention-ink">{fieldErrors['file']}</p>{/if}
			</div>
			<div class="grid gap-4 sm:grid-cols-2">
				<div class="grid gap-2">
					<Label for="asset-licence">Licence: who owns it and how it may be used</Label>
					<Input
						id="asset-licence"
						bind:value={licence}
						maxlength={200}
						placeholder="e.g. owned by the client"
						aria-invalid={fieldErrors['licence'] ? 'true' : undefined}
						aria-describedby={fieldErrors['licence'] ? 'asset-licence-error' : undefined}
					/>
					{#if fieldErrors['licence']}<p id="asset-licence-error" class="text-sm text-attention-ink">{fieldErrors['licence']}</p>{/if}
				</div>
				<div class="grid gap-2">
					<Label for="asset-source">Where it came from</Label>
					<Input
						id="asset-source"
						bind:value={source}
						maxlength={500}
						placeholder="e.g. sent by the owner by e-mail"
						aria-invalid={fieldErrors['source'] ? 'true' : undefined}
						aria-describedby={fieldErrors['source'] ? 'asset-source-error' : undefined}
					/>
					{#if fieldErrors['source']}<p id="asset-source-error" class="text-sm text-attention-ink">{fieldErrors['source']}</p>{/if}
				</div>
			</div>
			{#if result}
				<p role={result.ok ? 'status' : 'alert'} class={`rounded-lg border border-border px-4 py-3 text-sm ${result.ok ? 'bg-live-soft text-live-ink' : 'bg-attention-soft text-attention-ink'}`}>{result.text}</p>
			{/if}
			<div><Button type="submit" disabled={uploading || !storeOn}>{uploading ? 'Checking and saving…' : 'Upload'}</Button></div>
		</form>
	</section>

	{#if status === 'loading'}
		<ListSkeleton label="Loading assets" rows={2} />
	{:else if status === 'error'}
		<Alert.Root role="alert">
			<Alert.Title>Could not load the assets</Alert.Title>
			<Alert.Description>
				<p>{loadError}</p>
				<Button class="mt-3" variant="outline" size="sm" onclick={() => load(clients.selected)}>Try again</Button>
			</Alert.Description>
		</Alert.Root>
	{:else if assets.length === 0}
		<Empty.Root class="border border-dashed border-border">
			<Empty.Header>
				<Empty.Title>No files yet for {client.name}</Empty.Title>
				<Empty.Description>Start with the logo: upload it above with its licence.</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<ul class="grid gap-3 sm:grid-cols-2" aria-label={`Files for ${client.name}`}>
			{#each assets as asset (asset.id)}
				<li class="list-row flex gap-4 rounded-xl border border-border bg-card p-4">
					<div class="grid size-20 shrink-0 place-items-center overflow-hidden rounded-lg border border-border bg-white">
						{#if asset.media_type.startsWith('image/')}
							<img src={api.assetUrl(clients.selected, asset.id)} alt={`Preview of ${asset.original.name ?? 'the file'}`} class="max-h-full max-w-full object-contain" loading="lazy" />
						{:else}
							<span class="text-xs font-medium text-neutral-600">Font</span>
						{/if}
					</div>
					<div class="min-w-0 flex-1">
						<p class="truncate font-medium">{asset.original.name ?? 'Untitled file'}</p>
						<p class="text-sm text-muted-foreground">
							{kindOf(asset)} · {size(asset.bytes)}{#if asset.width && asset.height}{` · ${asset.width}×${asset.height}`}{/if}
						</p>
						{#if asset.removed_parts > 0}
							<Badge variant="pending" class="mt-1">{asset.removed_parts} unsafe part{asset.removed_parts === 1 ? '' : 's'} removed</Badge>
						{/if}
						<p class="mt-1 text-sm">Licence: {asset.licence}</p>
						<p class="text-sm text-muted-foreground">From: {asset.source} · {when(asset.created_at)}</p>
					</div>
				</li>
			{/each}
		</ul>
	{/if}
{/if}
