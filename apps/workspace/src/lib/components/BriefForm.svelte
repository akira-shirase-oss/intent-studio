<script lang="ts">
	import { untrack } from 'svelte';
	import { resolve } from '$app/paths';
	import { Button } from '@intent-studio/ui/ui/button';
	import { Checkbox } from '@intent-studio/ui/ui/checkbox';
	import { Input } from '@intent-studio/ui/ui/input';
	import { Label } from '@intent-studio/ui/ui/label';
	import * as NativeSelect from '@intent-studio/ui/ui/native-select';
	import { Textarea } from '@intent-studio/ui/ui/textarea';
	import Problems from '#lib/components/Problems.svelte';
	import type { Asset, Brief, Fact, Permission, Problem } from '#lib/api.ts';
	import { kindLabels, permissionLabels, problemsByPath } from '#lib/words.ts';

	let {
		initial,
		facts,
		assets,
		problems,
		saving,
		submitLabel,
		onsave
	}: {
		initial: Brief | null;
		facts: Fact[];
		assets: Asset[];
		problems: Problem[];
		saving: boolean;
		submitLabel: string;
		onsave: (brief: unknown) => void;
	} = $props();

	// The form's own copy, seeded once from `initial` (the page re-creates the form for a new
	// revision).
	const seed = untrack(() => initial);
	let businessName = $state(seed?.business_name ?? '');
	let intent = $state(seed?.intent ?? '');
	let audience = $state(seed?.audience ?? '');
	let services = $state(seed?.services.map((s) => ({ name: s.name, description: s.description ?? '' })) ?? [{ name: '', description: '' }]);
	let tone = $state(seed?.brand.tone ?? '');
	let colors = $state<string[]>(seed?.brand.colors ?? []);
	let logo = $state(seed?.brand.logo ?? '');
	let domains = $state((seed?.domains ?? []).join('\n'));
	let references = $state(
		seed?.references.map((r) => ({ url: r.url, permission: r.permission, notes: r.notes ?? '' })) ?? []
	);
	let claims = $state<string[]>(seed?.claims ?? []);

	const usable = $derived(facts.filter((f) => f.status === 'approved'));
	const unusable = $derived(facts.filter((f) => f.status === 'proposed' || f.status === 'stale').length);
	const images = $derived(assets.filter((a) => a.media_type.startsWith('image/')));
	const messages = $derived(problemsByPath(problems));
	const permissions = Object.keys(permissionLabels) as Permission[];

	function label(path: string): string {
		const named: Record<string, string> = {
			business_name: 'Business name',
			goal: 'Goal',
			intent: 'What the client wants',
			audience: 'Who it is for',
			services: 'Services',
			'brand.tone': 'Tone',
			'brand.logo': 'Logo',
			domains: 'Own web addresses',
			references: 'Reference sites',
			claims: 'Approved claims'
		};
		if (named[path]) return named[path];
		const match = /^(services|references|domains|claims|brand\.colors)\[(\d+)\](?:\.(\w+))?$/.exec(path);
		if (match) {
			const n = Number(match[2]) + 1;
			const part = match[3] ? ` (${match[3]})` : '';
			const what: Record<string, string> = { services: 'Service', references: 'Reference', domains: 'Web address', claims: 'Claim', 'brand.colors': 'Colour' };
			return `${what[match[1] ?? ''] ?? 'Item'} ${n}${part}`;
		}
		return path || 'The brief';
	}

	function fieldId(path: string): string | null {
		const direct: Record<string, string> = {
			business_name: 'brief-business-name',
			intent: 'brief-intent',
			audience: 'brief-audience',
			services: 'service-name-0',
			'brand.tone': 'brief-tone',
			'brand.logo': 'brief-logo',
			domains: 'brief-domains',
			claims: 'brief-claims'
		};
		if (direct[path]) return direct[path];
		const match = /^(services|references|domains|claims|brand\.colors)\[(\d+)\](?:\.(\w+))?$/.exec(path);
		if (!match) return null;
		const [, list, index, part] = match;
		if (list === 'services') return `service-${part ?? 'name'}-${index}`;
		if (list === 'references') return `reference-${part ?? 'url'}-${index}`;
		if (list === 'brand.colors') return `brief-color-${index}`;
		return list === 'domains' ? 'brief-domains' : 'brief-claims';
	}

	function err(path: string): { invalid: 'true' | undefined; describedby: string | undefined; id: string } {
		const id = `${fieldId(path) ?? path}-error`;
		return { invalid: messages.has(path) ? 'true' : undefined, describedby: messages.has(path) ? id : undefined, id };
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		onsave({
			business_name: businessName,
			goal: 'quote_requests',
			intent,
			audience,
			services: services.map((s) => ({ name: s.name, description: s.description || null })),
			brand: { tone: tone || null, colors, logo: logo || null },
			domains: domains.split(/[\s,]+/).filter(Boolean),
			references: references.map((r) => ({ url: r.url, permission: r.permission, notes: r.notes || null })),
			claims
		});
	}

	function toggleClaim(id: string, on: boolean) {
		claims = on ? [...claims, id] : claims.filter((c) => c !== id);
	}
</script>

{#snippet message(path: string)}
	{#if messages.has(path)}<p id={err(path).id} class="text-sm text-attention-ink">{messages.get(path)}</p>{/if}
{/snippet}

{#if problems.length > 0}
	<div class="max-w-3xl"><Problems title="The brief was not saved" {problems} {label} {fieldId} /></div>
{/if}

<form class="grid max-w-3xl gap-8" onsubmit={submit} novalidate>
	<section aria-labelledby="brief-basics" class="grid gap-4 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="brief-basics" class="text-lg font-semibold">The business and what it wants</h2>
		<p class="text-sm text-muted-foreground">The site’s goal is <strong>quote requests</strong> — the one site pattern this release builds.</p>
		<div class="grid gap-2">
			<Label for="brief-business-name">Business name, as the site should say it</Label>
			<Input id="brief-business-name" bind:value={businessName} maxlength={120} autocomplete="organization" aria-invalid={err('business_name').invalid} aria-describedby={err('business_name').describedby} />
			{@render message('business_name')}
		</div>
		<div class="grid gap-2">
			<Label for="brief-intent">What the client wants the site to achieve</Label>
			<Textarea id="brief-intent" bind:value={intent} rows={3} maxlength={1000} aria-invalid={err('intent').invalid} aria-describedby={err('intent').describedby} />
			{@render message('intent')}
		</div>
		<div class="grid gap-2">
			<Label for="brief-audience">Who it is for</Label>
			<Textarea id="brief-audience" bind:value={audience} rows={2} maxlength={1000} aria-invalid={err('audience').invalid} aria-describedby={err('audience').describedby} />
			{@render message('audience')}
		</div>
	</section>

	<section aria-labelledby="brief-services" class="grid gap-4 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="brief-services" class="text-lg font-semibold">Services</h2>
		<p class="text-sm text-muted-foreground">What the business offers. The quote form offers these. Prices are facts, not part of the brief.</p>
		{@render message('services')}
		{#each services as service, i (i)}
			<fieldset class="grid gap-3 rounded-lg border border-border p-4">
				<legend class="px-1 text-sm font-medium">Service {i + 1}</legend>
				<div class="grid gap-2">
					<Label for={`service-name-${i}`}>Name</Label>
					<Input id={`service-name-${i}`} bind:value={service.name} maxlength={80} aria-invalid={err(`services[${i}].name`).invalid} aria-describedby={err(`services[${i}].name`).describedby} />
					{@render message(`services[${i}].name`)}
				</div>
				<div class="grid gap-2">
					<Label for={`service-description-${i}`}>What it involves (optional)</Label>
					<Textarea id={`service-description-${i}`} bind:value={service.description} rows={2} maxlength={500} />
					{@render message(`services[${i}].description`)}
				</div>
				{#if services.length > 1}
					<div><Button size="sm" variant="ghost" onclick={() => (services = services.filter((_, j) => j !== i))}>Remove service {i + 1}</Button></div>
				{/if}
			</fieldset>
		{/each}
		{#if services.length < 20}
			<div><Button variant="outline" size="sm" onclick={() => (services = [...services, { name: '', description: '' }])}>Add a service</Button></div>
		{/if}
	</section>

	<section aria-labelledby="brief-brand" class="grid gap-4 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="brief-brand" class="text-lg font-semibold">Look and tone</h2>
		<div class="grid gap-2">
			<Label for="brief-tone">How the site should sound (optional)</Label>
			<Input id="brief-tone" bind:value={tone} maxlength={200} placeholder="e.g. plain and friendly, no jargon" />
			{@render message('brand.tone')}
		</div>
		<fieldset class="grid gap-3">
			<legend class="mb-1 text-sm font-medium">Brand colours (optional, up to 6, most important first)</legend>
			<div class="flex flex-wrap gap-3">
				{#each colors as color, i (i)}
					<div class="flex items-center gap-2">
						<Label for={`brief-color-${i}`} class="sr-only">Colour {i + 1}</Label>
						<input
							id={`brief-color-${i}`}
							type="color"
							bind:value={colors[i]}
							class="h-10 w-14 cursor-pointer rounded-lg border border-input bg-card outline-none focus-visible:ring-4 focus-visible:ring-ring/25"
						/>
						<span class="font-mono text-sm">{color}</span>
						<Button size="sm" variant="ghost" onclick={() => (colors = colors.filter((_, j) => j !== i))}>Remove colour {i + 1}</Button>
					</div>
				{/each}
			</div>
			{#if colors.length < 6}
				<div><Button size="sm" variant="outline" onclick={() => (colors = [...colors, '#1f4e79'])}>Add a colour</Button></div>
			{/if}
		</fieldset>
		<div class="grid gap-2">
			<Label for="brief-logo">Logo (optional)</Label>
			<NativeSelect.Root id="brief-logo" class="w-full sm:w-96" bind:value={logo} aria-describedby="brief-logo-hint">
				<NativeSelect.Option value="">No logo yet</NativeSelect.Option>
				{#each images as image (image.id)}
					<NativeSelect.Option value={image.id}>{image.original.name ?? image.id} ({image.media_type.replace('image/', '').toUpperCase()})</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<p id="brief-logo-hint" class="text-sm text-muted-foreground">Upload the client’s logo on <a class="underline underline-offset-4" href={resolve('/assets')}>Assets</a> first.</p>
			{@render message('brand.logo')}
		</div>
	</section>

	<section aria-labelledby="brief-web" class="grid gap-4 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="brief-web" class="text-lg font-semibold">On the web</h2>
		<div class="grid gap-2">
			<Label for="brief-domains">The client’s own web addresses (optional, one per line)</Label>
			<Textarea id="brief-domains" bind:value={domains} rows={2} placeholder="leeheating.co.uk" aria-describedby="brief-domains-hint" aria-invalid={[...messages.keys()].some((k) => k.startsWith('domains')) ? 'true' : undefined} />
			<p id="brief-domains-hint" class="text-sm text-muted-foreground">Pages may link only to these and to approved contact details.</p>
			{#each [...messages.keys()].filter((k) => k.startsWith('domains')) as path (path)}<p class="text-sm text-attention-ink">{label(path)}: {messages.get(path)}</p>{/each}
		</div>
		<fieldset class="grid gap-3">
			<legend class="mb-1 text-sm font-medium">Sites the client pointed at (optional, up to 10)</legend>
			{#each references as reference, i (i)}
				<div class="grid gap-3 rounded-lg border border-border p-4">
					<div class="grid gap-2">
						<Label for={`reference-url-${i}`}>Reference {i + 1}: address</Label>
						<Input id={`reference-url-${i}`} type="url" bind:value={reference.url} placeholder="https://" aria-invalid={err(`references[${i}].url`).invalid} aria-describedby={err(`references[${i}].url`).describedby} />
						{@render message(`references[${i}].url`)}
					</div>
					<div class="grid gap-2">
						<Label for={`reference-permission-${i}`}>What we may do with it</Label>
						<NativeSelect.Root id={`reference-permission-${i}`} class="w-full" bind:value={reference.permission}>
							{#each permissions as p (p)}<NativeSelect.Option value={p}>{permissionLabels[p]}</NativeSelect.Option>{/each}
						</NativeSelect.Root>
					</div>
					<div class="grid gap-2">
						<Label for={`reference-notes-${i}`}>Notes: what to take from it{reference.permission === 'licensed_assets' ? ', and the licence' : ''}</Label>
						<Textarea id={`reference-notes-${i}`} bind:value={reference.notes} rows={2} maxlength={2000} aria-invalid={err(`references[${i}].notes`).invalid} aria-describedby={err(`references[${i}].notes`).describedby} />
						{@render message(`references[${i}].notes`)}
					</div>
					<div><Button size="sm" variant="ghost" onclick={() => (references = references.filter((_, j) => j !== i))}>Remove reference {i + 1}</Button></div>
				</div>
			{/each}
			{#if references.length < 10}
				<div><Button size="sm" variant="outline" onclick={() => (references = [...references, { url: '', permission: 'inspiration_only', notes: '' }])}>Add a reference site</Button></div>
			{/if}
		</fieldset>
	</section>

	<section aria-labelledby="brief-claims-title" class="grid gap-4 rounded-xl border border-border bg-card p-4 sm:p-6">
		<h2 id="brief-claims-title" class="text-lg font-semibold">Approved claims</h2>
		<p class="text-sm text-muted-foreground">
			The facts this site may state. Only approved, current facts can be chosen{#if unusable > 0}{`; ${unusable} more ${unusable === 1 ? 'waits' : 'wait'} for approval or re-confirmation on `}
				<a class="underline underline-offset-4" href={resolve('/facts')}>Facts</a>{/if}.
		</p>
		{#each [...messages.keys()].filter((k) => k.startsWith('claims')) as path (path)}<p class="text-sm text-attention-ink" role="alert">{label(path)}: {messages.get(path)}</p>{/each}
		{#if usable.length === 0}
			<p id="brief-claims" class="rounded-lg border border-dashed border-border p-4 text-sm text-muted-foreground">
				No approved facts yet. The brief can be saved without them and revised later.
			</p>
		{:else}
			<ul id="brief-claims" class="grid gap-2">
				{#each usable as fact (fact.id)}
					<li class="flex items-start gap-3 rounded-lg border border-border p-3">
						<Checkbox id={`claim-${fact.id}`} checked={claims.includes(fact.id)} onCheckedChange={(on) => toggleClaim(fact.id, on === true)} />
						<Label for={`claim-${fact.id}`} class="grid gap-0.5 font-normal">
							<span class="text-xs font-medium text-muted-foreground">{kindLabels[fact.kind]}</span>
							<span>{fact.statement}</span>
						</Label>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<div class="flex flex-wrap gap-3">
		<Button type="submit" disabled={saving}>{saving ? 'Saving…' : submitLabel}</Button>
		<Button variant="ghost" href={resolve('/briefs')}>Cancel</Button>
	</div>
</form>
