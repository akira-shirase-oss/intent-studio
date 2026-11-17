<script lang="ts">
	import { Badge } from '@intent-studio/ui/ui/badge';
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Alert from '@intent-studio/ui/ui/alert';
	import * as Empty from '@intent-studio/ui/ui/empty';
	import { Input } from '@intent-studio/ui/ui/input';
	import { Label } from '@intent-studio/ui/ui/label';
	import * as NativeSelect from '@intent-studio/ui/ui/native-select';
	import { Textarea } from '@intent-studio/ui/ui/textarea';
	import ClientBar from '#lib/components/ClientBar.svelte';
	import ListSkeleton from '#lib/components/ListSkeleton.svelte';
	import Problems from '#lib/components/Problems.svelte';
	import { ApiError, api, type Fact, type FactKind, type Problem } from '#lib/api.ts';
	import { clients, loadClients } from '#lib/clients.svelte.ts';
	import { sessionEnded } from '#lib/session.svelte.ts';
	import { day, factStatus, failureText, kindHints, kindLabels, problemsByPath, todayUtc } from '#lib/words.ts';

	type Filter = 'all' | Fact['status'];

	let status = $state<'loading' | 'ready' | 'error'>('loading');
	let facts = $state<Fact[]>([]);
	let loadError = $state('');
	let filter = $state<Filter>('all');
	let notice = $state<string | null>(null);

	// The add form.
	let adding = $state(false);
	let kind = $state<FactKind>('testimonial');
	let statement = $state('');
	let source = $state('');
	let linkKind = $state<'' | 'tel' | 'mailto' | 'url'>('');
	let linkValue = $state('');
	let addProblems = $state<Problem[]>([]);
	let addError = $state<string | null>(null);
	let saving = $state(false);

	// One open action at a time: approving or withdrawing a fact.
	let action = $state<{ id: string; kind: 'approve' | 'withdraw' } | null>(null);
	let approvedBy = $state('');
	let approvedOn = $state(todayUtc());
	let reviewBy = $state('');
	let actionProblems = $state<Problem[]>([]);
	let actionError = $state<string | null>(null);

	const kinds = Object.keys(kindLabels) as FactKind[];
	let kindSelect = $state<HTMLSelectElement | null>(null);

	// Opening the form moves focus into it, so a keyboard user carries on from the button.
	$effect(() => {
		if (adding && kindSelect) kindSelect.focus();
	});
	const shown = $derived(filter === 'all' ? facts : facts.filter((f) => f.status === filter));
	const addMessages = $derived(problemsByPath(addProblems));
	const actionMessages = $derived(problemsByPath(actionProblems));
	const client = $derived(clients.list.find((c) => c.id === clients.selected));

	const fieldIds: Record<string, string> = {
		kind: 'fact-kind',
		statement: 'fact-statement',
		source: 'fact-source',
		link: 'fact-link-value',
		'link.tel': 'fact-link-value',
		'link.mailto': 'fact-link-value',
		'link.url': 'fact-link-value'
	};
	const fieldLabels: Record<string, string> = {
		kind: 'Kind',
		statement: 'What it says',
		source: 'Where it came from',
		link: 'Link',
		'link.tel': 'Phone number',
		'link.mailto': 'E-mail address',
		'link.url': 'Web address',
		approved_by: 'Approved by',
		approved_on: 'Approved on',
		review_by: 'Review by'
	};

	async function load(tenant: string) {
		status = 'loading';
		try {
			facts = (await api.facts(tenant)).facts;
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

	function resetAdd() {
		statement = '';
		source = '';
		linkKind = '';
		linkValue = '';
		addProblems = [];
		addError = null;
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		saving = true;
		addProblems = [];
		addError = null;
		try {
			const fact = await api.createFact(clients.selected, {
				kind,
				statement,
				source,
				...(linkKind && linkValue.trim() ? { link: { [linkKind]: linkValue } } : {})
			});
			facts = [fact, ...facts];
			notice = `Recorded: ${kindLabels[fact.kind].toLowerCase()}, waiting for the client’s approval.`;
			resetAdd();
			adding = false;
		} catch (error) {
			if (sessionEnded(error)) return;
			if (error instanceof ApiError && error.problems.length > 0) addProblems = error.problems;
			else addError = failureText(error, true);
		} finally {
			saving = false;
		}
	}

	function open(id: string, which: 'approve' | 'withdraw') {
		action = { id, kind: which };
		approvedBy = '';
		approvedOn = todayUtc();
		reviewBy = '';
		actionProblems = [];
		actionError = null;
	}

	function replace(updated: Fact) {
		facts = facts.map((f) => (f.id === updated.id ? updated : f));
	}

	async function approve(event: SubmitEvent, fact: Fact) {
		event.preventDefault();
		saving = true;
		actionProblems = [];
		actionError = null;
		try {
			const updated = await api.approveFact(clients.selected, fact.id, {
				approved_by: approvedBy,
				approved_on: approvedOn,
				...(reviewBy ? { review_by: reviewBy } : {})
			});
			replace(updated);
			notice = updated.status === 'stale'
				? `Approval recorded, but it already ran out after ${day(updated.review_by)}: re-confirm it with the client.`
				: `Approved until ${day(updated.review_by)}.`;
			action = null;
		} catch (error) {
			if (sessionEnded(error)) return;
			if (error instanceof ApiError && error.problems.length > 0) actionProblems = error.problems;
			else actionError = failureText(error, true);
		} finally {
			saving = false;
		}
	}

	async function withdraw(fact: Fact) {
		saving = true;
		actionError = null;
		try {
			replace(await api.withdrawFact(clients.selected, fact.id));
			notice = 'Withdrawn. Pages can no longer use this fact.';
			action = null;
		} catch (error) {
			if (sessionEnded(error)) return;
			actionError = failureText(error, true);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>Facts · Intent Studio</title></svelte:head>

<div class="mb-4">
	<h1 class="text-2xl font-semibold tracking-tight">Facts</h1>
	<p class="mt-1 max-w-2xl text-sm text-muted-foreground">
		Every claim a page makes — a review, a price, a licence, opening hours — is a fact the client approved. Approvals last 90 days
		(30 for prices and hours); after that the fact needs re-confirming before a page can use it.
	</p>
</div>

<ClientBar />

{#if clients.status === 'ready' && client}
	{#if notice}
		<p role="status" class="mb-4 rounded-xl border border-border bg-live-soft px-4 py-3 text-sm text-live-ink">{notice}</p>
	{/if}

	<div class="mb-4 flex flex-wrap items-end justify-between gap-3">
		<div class="grid gap-2">
			<Label for="fact-filter">Show</Label>
			<NativeSelect.Root id="fact-filter" bind:value={filter}>
				<NativeSelect.Option value="all">All facts</NativeSelect.Option>
				<NativeSelect.Option value="proposed">Waiting for approval</NativeSelect.Option>
				<NativeSelect.Option value="approved">Approved</NativeSelect.Option>
				<NativeSelect.Option value="stale">Approval ran out</NativeSelect.Option>
				<NativeSelect.Option value="withdrawn">Withdrawn</NativeSelect.Option>
			</NativeSelect.Root>
		</div>
		{#if !adding}
			<Button onclick={() => { adding = true; notice = null; }}>Add a fact</Button>
		{/if}
	</div>

	{#if adding}
		<section aria-labelledby="add-fact-title" class="mb-6 rounded-xl border border-border bg-card p-4 sm:p-6">
			<h2 id="add-fact-title" class="text-lg font-semibold">Add a fact for {client.name}</h2>
			<p class="mt-1 text-sm text-muted-foreground">It is recorded as waiting for approval; pages cannot use it until the client approves it.</p>
			{#if addProblems.length > 0}
				<div class="mt-4">
					<Problems title="The fact was not saved" problems={addProblems} label={(p) => fieldLabels[p] ?? p} fieldId={(p) => fieldIds[p] ?? null} />
				</div>
			{/if}
			{#if addError}<p role="alert" class="mt-4 text-sm text-attention-ink">{addError}</p>{/if}
			<form class="mt-4 grid gap-4" onsubmit={add}>
				<div class="grid gap-2">
					<Label for="fact-kind">Kind</Label>
					<NativeSelect.Root id="fact-kind" class="w-full sm:w-80" bind:ref={kindSelect} bind:value={kind} aria-describedby="fact-kind-hint">
						{#each kinds as k (k)}
							<NativeSelect.Option value={k}>{kindLabels[k]}</NativeSelect.Option>
						{/each}
					</NativeSelect.Root>
					<p id="fact-kind-hint" class="text-sm text-muted-foreground">{kindHints[kind]}</p>
				</div>
				<div class="grid gap-2">
					<Label for="fact-statement">What it says</Label>
					<Textarea
						id="fact-statement"
						bind:value={statement}
						maxlength={1000}
						rows={3}
						required
						aria-invalid={addMessages.has('statement') ? 'true' : undefined}
						aria-describedby={addMessages.has('statement') ? 'fact-statement-error' : undefined}
					/>
					{#if addMessages.has('statement')}<p id="fact-statement-error" class="text-sm text-attention-ink">{addMessages.get('statement')}</p>{/if}
				</div>
				<div class="grid gap-2">
					<Label for="fact-source">Where it came from</Label>
					<Input
						id="fact-source"
						bind:value={source}
						maxlength={500}
						required
						placeholder="e.g. the owner’s e-mail of 10 November"
						aria-invalid={addMessages.has('source') ? 'true' : undefined}
						aria-describedby={addMessages.has('source') ? 'fact-source-error' : undefined}
					/>
					{#if addMessages.has('source')}<p id="fact-source-error" class="text-sm text-attention-ink">{addMessages.get('source')}</p>{/if}
				</div>
				{#if kind === 'contact_details'}
					<fieldset class="grid gap-2 sm:grid-cols-[auto_1fr] sm:items-end sm:gap-3">
						<legend class="mb-2 text-sm font-medium">Link (optional): where a page’s button may go</legend>
						<div class="grid gap-2">
							<Label for="fact-link-kind">Type</Label>
							<NativeSelect.Root id="fact-link-kind" bind:value={linkKind}>
								<NativeSelect.Option value="">No link</NativeSelect.Option>
								<NativeSelect.Option value="tel">Phone number</NativeSelect.Option>
								<NativeSelect.Option value="mailto">E-mail address</NativeSelect.Option>
								<NativeSelect.Option value="url">Web address (https)</NativeSelect.Option>
							</NativeSelect.Root>
						</div>
						<div class="grid gap-2">
							<Label for="fact-link-value">Number or address</Label>
							<Input
								id="fact-link-value"
								bind:value={linkValue}
								disabled={!linkKind}
								aria-invalid={[...addMessages.keys()].some((k) => k.startsWith('link')) ? 'true' : undefined}
							/>
						</div>
					</fieldset>
				{/if}
				<div class="flex flex-wrap gap-3">
					<Button type="submit" disabled={saving}>{saving ? 'Saving…' : 'Save fact'}</Button>
					<Button variant="ghost" onclick={() => { adding = false; resetAdd(); }}>Cancel</Button>
				</div>
			</form>
		</section>
	{/if}

	{#if status === 'loading'}
		<ListSkeleton label="Loading facts" />
	{:else if status === 'error'}
		<Alert.Root role="alert">
			<Alert.Title>Could not load the facts</Alert.Title>
			<Alert.Description>
				<p>{loadError}</p>
				<Button class="mt-3" variant="outline" size="sm" onclick={() => load(clients.selected)}>Try again</Button>
			</Alert.Description>
		</Alert.Root>
	{:else if facts.length === 0}
		<Empty.Root class="border border-dashed border-border">
			<Empty.Header>
				<Empty.Title>No facts yet for {client.name}</Empty.Title>
				<Empty.Description>Start with what the site will say: a customer review, a price, the area they cover, how to reach them.</Empty.Description>
			</Empty.Header>
			{#if !adding}<Empty.Content><Button onclick={() => (adding = true)}>Add a fact</Button></Empty.Content>{/if}
		</Empty.Root>
	{:else if shown.length === 0}
		<p class="rounded-xl border border-dashed border-border p-6 text-sm text-muted-foreground">No facts match this filter.</p>
	{:else}
		<ul class="grid gap-3" aria-label={`Facts for ${client.name}`}>
			{#each shown as fact (fact.id)}
				{@const label = factStatus(fact)}
				<li class="list-row rounded-xl border border-border bg-card p-4">
					<div class="flex flex-wrap items-start justify-between gap-2">
						<p class="text-sm font-medium text-muted-foreground">{kindLabels[fact.kind]}</p>
						<Badge variant={label.variant}>{label.text}</Badge>
					</div>
					<p class="mt-2 whitespace-pre-line">{fact.statement}</p>
					{#if fact.link}<p class="mt-1 text-sm break-all text-muted-foreground">Links to {fact.link.href}</p>{/if}
					<p class="mt-2 text-sm text-muted-foreground">
						From: {fact.source}{#if fact.approved_by}{` · Approved by ${fact.approved_by}, on ${day(fact.approved_on)}`}{/if}
					</p>
					{#if fact.status !== 'withdrawn'}
						{#if action?.id === fact.id && action.kind === 'approve'}
							<form class="mt-4 grid gap-4 rounded-lg border border-border p-4" onsubmit={(event) => approve(event, fact)}>
								<p class="text-sm">Record the client’s approval. It lasts until the review date, at most {fact.review_days} days.</p>
								{#if actionProblems.length > 0}
									<Problems title="The approval was not recorded" problems={actionProblems} label={(p) => fieldLabels[p] ?? p} fieldId={(p) => `approval-${p}`} />
								{/if}
								<div class="grid gap-4 sm:grid-cols-3">
									<div class="grid gap-2">
										<Label for="approval-approved_by">Approved by</Label>
										<Input id="approval-approved_by" bind:value={approvedBy} required maxlength={200} placeholder="Name and role" aria-invalid={actionMessages.has('approved_by') ? 'true' : undefined} />
									</div>
									<div class="grid gap-2">
										<Label for="approval-approved_on">Approved on</Label>
										<Input id="approval-approved_on" type="date" bind:value={approvedOn} max={todayUtc()} required aria-invalid={actionMessages.has('approved_on') ? 'true' : undefined} />
									</div>
									<div class="grid gap-2">
										<Label for="approval-review_by">Review by (optional)</Label>
										<Input id="approval-review_by" type="date" bind:value={reviewBy} aria-describedby="approval-review-hint" aria-invalid={actionMessages.has('review_by') ? 'true' : undefined} />
										<p id="approval-review-hint" class="text-xs text-muted-foreground">Empty: the longest the kind allows.</p>
									</div>
								</div>
								{#if actionError}<p role="alert" class="text-sm text-attention-ink">{actionError}</p>{/if}
								<div class="flex flex-wrap gap-3">
									<Button type="submit" disabled={saving}>{saving ? 'Saving…' : 'Record approval'}</Button>
									<Button variant="ghost" onclick={() => (action = null)}>Cancel</Button>
								</div>
							</form>
						{:else if action?.id === fact.id && action.kind === 'withdraw'}
							<div class="mt-4 grid gap-3 rounded-lg border border-border p-4" role="group" aria-label="Withdraw this fact">
								<p class="text-sm">Withdraw this fact? Pages will never be able to use it again; to change what it says, add a new fact.</p>
								{#if actionError}<p role="alert" class="text-sm text-attention-ink">{actionError}</p>{/if}
								<div class="flex flex-wrap gap-3">
									<Button variant="destructive" disabled={saving} onclick={() => withdraw(fact)}>{saving ? 'Withdrawing…' : 'Withdraw'}</Button>
									<Button variant="ghost" onclick={() => (action = null)}>Keep it</Button>
								</div>
							</div>
						{:else}
							<div class="mt-3 flex flex-wrap gap-2">
								<Button size="sm" variant="outline" onclick={() => open(fact.id, 'approve')}>
									{fact.status === 'proposed' ? 'Record approval' : 'Re-confirm'}
								</Button>
								<Button size="sm" variant="ghost" onclick={() => open(fact.id, 'withdraw')}>Withdraw</Button>
							</div>
						{/if}
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
{/if}
