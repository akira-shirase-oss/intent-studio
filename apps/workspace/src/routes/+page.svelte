<script lang="ts">
	import { onMount } from 'svelte';
	import * as Alert from '@intent-studio/ui/ui/alert';
	import { Badge } from '@intent-studio/ui/ui/badge';
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Card from '@intent-studio/ui/ui/card';
	import * as Empty from '@intent-studio/ui/ui/empty';
	import { Input } from '@intent-studio/ui/ui/input';
	import { Label } from '@intent-studio/ui/ui/label';
	import * as Table from '@intent-studio/ui/ui/table';
	import { ApiError, api, notificationLabel, type InboxLead } from '#lib/api.ts';

	type View = 'loading' | 'signed-out' | 'inbox';

	let view = $state<View>('loading');
	let password = $state('');
	let signInError = $state<string | null>(null);
	let busy = $state(false);
	let leads = $state<InboxLead[]>([]);
	let nextBefore = $state<string | null>(null);
	let loadError = $state<string | null>(null);

	const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });

	function describe(error: unknown): string {
		if (error instanceof ApiError) {
			if (error.code === 'unavailable') return 'The database is unavailable right now. Leads already saved are safe; try again in a moment.';
			return error.message;
		}
		return 'The server could not be reached.';
	}

	async function loadFirstPage() {
		loadError = null;
		try {
			const page = await api.leads();
			leads = page.leads;
			nextBefore = page.next_before;
			view = 'inbox';
		} catch (error) {
			if (error instanceof ApiError && error.status === 401) {
				view = 'signed-out';
			} else {
				loadError = describe(error);
				view = 'inbox';
			}
		}
	}

	async function loadMore() {
		if (!nextBefore) return;
		busy = true;
		try {
			const page = await api.leads(nextBefore);
			leads = [...leads, ...page.leads];
			nextBefore = page.next_before;
		} catch (error) {
			loadError = describe(error);
		} finally {
			busy = false;
		}
	}

	async function signIn(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		signInError = null;
		try {
			await api.signIn(password);
			password = '';
			await loadFirstPage();
		} catch (error) {
			signInError =
				error instanceof ApiError && error.status === 401
					? 'That password is not right.'
					: error instanceof ApiError && error.code === 'rate_limited'
						? 'Too many attempts. Wait a minute and try again.'
						: describe(error);
		} finally {
			busy = false;
		}
	}

	async function signOut() {
		await api.signOut().catch(() => undefined);
		leads = [];
		view = 'signed-out';
	}

	function contact(lead: InboxLead): string {
		return [lead.email, lead.phone].filter(Boolean).join(' · ');
	}

	onMount(loadFirstPage);
</script>

<div class="min-h-dvh bg-background text-foreground">
	<header class="border-b border-border">
		<div class="mx-auto flex max-w-6xl items-center justify-between gap-4 px-6 py-4">
			<div>
				<p class="text-sm text-muted-foreground">Intent Studio</p>
				<h1 class="text-xl font-semibold tracking-tight">Lead inbox</h1>
			</div>
			{#if view === 'inbox'}
				<div class="flex items-center gap-2">
					<Button variant="outline" size="sm" onclick={loadFirstPage}>Refresh</Button>
					<Button variant="ghost" size="sm" onclick={signOut}>Sign out</Button>
				</div>
			{/if}
		</div>
	</header>

	<main class="mx-auto max-w-6xl px-6 py-8">
		{#if view === 'loading'}
			<p class="text-sm text-muted-foreground" aria-live="polite">Loading…</p>
		{:else if view === 'signed-out'}
			<Card.Root class="mx-auto max-w-sm">
				<Card.Header>
					<Card.Title><h2 class="text-inherit">Operator sign-in</h2></Card.Title>
					<Card.Description>The inbox holds every lead the sites have received.</Card.Description>
				</Card.Header>
				<Card.Content>
					<form class="grid gap-4" onsubmit={signIn}>
						<div class="grid gap-2">
							<Label for="password">Password</Label>
							<Input
								id="password"
								type="password"
								autocomplete="current-password"
								required
								bind:value={password}
								aria-invalid={signInError ? 'true' : undefined}
								aria-describedby={signInError ? 'sign-in-error' : undefined}
							/>
						</div>
						{#if signInError}
							<p id="sign-in-error" class="text-sm text-attention-ink" role="alert">{signInError}</p>
						{/if}
						<Button type="submit" disabled={busy || password.length === 0}>Sign in</Button>
					</form>
				</Card.Content>
			</Card.Root>
		{:else}
			{#if loadError}
				<Alert.Root class="mb-6" role="alert">
					<Alert.Title>Could not load the inbox</Alert.Title>
					<Alert.Description>{loadError}</Alert.Description>
				</Alert.Root>
			{/if}
			{#if leads.length === 0 && !loadError}
				<Empty.Root>
					<Empty.Header>
						<Empty.Title>No leads yet</Empty.Title>
						<Empty.Description>When a site's form is submitted, the lead appears here before anyone is notified.</Empty.Description>
					</Empty.Header>
				</Empty.Root>
			{:else if leads.length > 0}
				<!-- Phones: one card per lead, every field visible without sideways scrolling. -->
				<ul class="grid gap-3 md:hidden" aria-label="Leads, newest first">
					{#each leads as lead (lead.id)}
						{@const label = notificationLabel(lead)}
						<li class="rounded-xl border border-border bg-card p-4">
							<div class="flex flex-wrap items-start justify-between gap-2">
								<p class="font-medium">
									{lead.name}
									{#if lead.is_test}<Badge variant="outline" class="ml-2">Test</Badge>{/if}
								</p>
								<Badge variant={label.variant}>{label.text}</Badge>
							</div>
							<p class="mt-1 text-sm text-muted-foreground">
								<time datetime={lead.received_at}>{dateTime.format(new Date(lead.received_at))}</time> · {lead.site_name}
							</p>
							<p class="mt-2 text-sm break-all">{contact(lead)}</p>
							{#if lead.service || lead.area}
								<p class="mt-2 text-sm text-muted-foreground">{[lead.service, lead.area].filter(Boolean).join(' · ')}</p>
							{/if}
							{#if lead.message}
								<p class="mt-1 text-sm whitespace-pre-line">{lead.message}</p>
							{/if}
						</li>
					{/each}
				</ul>
				<div class="hidden md:block">
				<Table.Root>
					<Table.Caption class="sr-only">Leads, newest first</Table.Caption>
					<Table.Header>
						<Table.Row>
							<Table.Head>Received</Table.Head>
							<Table.Head>Site</Table.Head>
							<Table.Head>Name</Table.Head>
							<Table.Head>Contact</Table.Head>
							<Table.Head>Request</Table.Head>
							<Table.Head>Notification</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each leads as lead (lead.id)}
							{@const label = notificationLabel(lead)}
							<Table.Row>
								<Table.Cell class="whitespace-nowrap tabular-nums">
									<time datetime={lead.received_at}>{dateTime.format(new Date(lead.received_at))}</time>
								</Table.Cell>
								<Table.Cell>{lead.site_name}</Table.Cell>
								<Table.Cell class="font-medium">
									{lead.name}
									{#if lead.is_test}<Badge variant="outline" class="ml-2">Test</Badge>{/if}
								</Table.Cell>
								<Table.Cell class="break-all">{contact(lead)}</Table.Cell>
								<Table.Cell class="max-w-sm">
									{#if lead.service || lead.area}
										<p class="text-sm text-muted-foreground">{[lead.service, lead.area].filter(Boolean).join(' · ')}</p>
									{/if}
									<p class="line-clamp-2 whitespace-pre-line" title={lead.message}>{lead.message}</p>
								</Table.Cell>
								<Table.Cell><Badge variant={label.variant}>{label.text}</Badge></Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
				</div>
				{#if nextBefore}
					<div class="mt-6 flex justify-center">
						<Button variant="outline" onclick={loadMore} disabled={busy}>Show older leads</Button>
					</div>
				{/if}
			{/if}
		{/if}
	</main>
</div>
