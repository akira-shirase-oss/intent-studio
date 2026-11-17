<script lang="ts">
	import { onMount } from 'svelte';
	import * as Alert from '@intent-studio/ui/ui/alert';
	import { Badge } from '@intent-studio/ui/ui/badge';
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Empty from '@intent-studio/ui/ui/empty';
	import * as Table from '@intent-studio/ui/ui/table';
	import ListSkeleton from '#lib/components/ListSkeleton.svelte';
	import { ApiError, api, notificationLabel, type InboxLead } from '#lib/api.ts';
	import { sessionEnded } from '#lib/session.svelte.ts';
	import { failureText } from '#lib/words.ts';

	let status = $state<'loading' | 'ready'>('loading');
	let leads = $state<InboxLead[]>([]);
	let nextBefore = $state<string | null>(null);
	let loadError = $state<string | null>(null);
	let busy = $state(false);

	const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });

	function describe(error: unknown): string {
		if (error instanceof ApiError && error.code === 'unavailable') {
			return 'The database is unavailable right now. Leads already saved are safe; try again in a moment.';
		}
		return failureText(error);
	}

	async function loadFirstPage() {
		loadError = null;
		busy = true;
		try {
			const page = await api.leads();
			leads = page.leads;
			nextBefore = page.next_before;
		} catch (error) {
			if (sessionEnded(error)) return;
			loadError = describe(error);
		} finally {
			status = 'ready';
			busy = false;
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
			if (sessionEnded(error)) return;
			loadError = describe(error);
		} finally {
			busy = false;
		}
	}

	function contact(lead: InboxLead): string {
		return [lead.email, lead.phone].filter(Boolean).join(' · ');
	}

	onMount(loadFirstPage);
</script>

<svelte:head><title>Lead inbox · Intent Studio</title></svelte:head>

<div class="mb-6 flex flex-wrap items-end justify-between gap-3">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">Lead inbox</h1>
		<p class="mt-1 text-sm text-muted-foreground">Every lead the sites have received, newest first. Each is saved here before any notification is sent.</p>
	</div>
	<Button variant="outline" size="sm" onclick={loadFirstPage} disabled={busy}>Refresh</Button>
</div>

{#if loadError}
	<Alert.Root class="mb-6" role="alert">
		<Alert.Title>Could not load the inbox</Alert.Title>
		<Alert.Description>
			<p>{loadError}</p>
			<Button class="mt-3" variant="outline" size="sm" onclick={loadFirstPage}>Try again</Button>
		</Alert.Description>
	</Alert.Root>
{/if}

{#if status === 'loading'}
	<ListSkeleton label="Loading leads" />
{:else if leads.length === 0 && !loadError}
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
			<li class="list-row rounded-xl border border-border bg-card p-4">
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
