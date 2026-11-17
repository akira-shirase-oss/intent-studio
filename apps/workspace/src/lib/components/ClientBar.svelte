<script lang="ts">
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Empty from '@intent-studio/ui/ui/empty';
	import { Input } from '@intent-studio/ui/ui/input';
	import { Label } from '@intent-studio/ui/ui/label';
	import * as NativeSelect from '@intent-studio/ui/ui/native-select';
	import { Skeleton } from '@intent-studio/ui/ui/skeleton';
	import { ApiError } from '#lib/api.ts';
	import { addClient, clients, loadClients, selectClient } from '#lib/clients.svelte.ts';
	import { failureText, problemText } from '#lib/words.ts';

	let adding = $state(false);
	let name = $state('');
	let error = $state<string | null>(null);
	let busy = $state(false);
	let nameInput = $state<HTMLInputElement | null>(null);

	// Opening the form moves focus into it, so a keyboard user types straight away.
	$effect(() => {
		if (adding && nameInput) nameInput.focus();
	});

	async function add(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			await addClient(name);
			name = '';
			adding = false;
		} catch (failure) {
			error = failure instanceof ApiError && failure.problems[0] ? problemText(failure.problems[0]) : failureText(failure, true);
		} finally {
			busy = false;
		}
	}
</script>

{#snippet addForm()}
	<form class="flex flex-wrap items-end gap-3" onsubmit={add}>
		<div class="grid min-w-56 flex-1 gap-2">
			<Label for="new-client">Client’s business name</Label>
			<Input
				id="new-client"
				bind:ref={nameInput}
				bind:value={name}
				maxlength={200}
				required
				autocomplete="organization"
				aria-invalid={error ? 'true' : undefined}
				aria-describedby={error ? 'new-client-error' : undefined}
			/>
		</div>
		<Button type="submit" disabled={busy || name.trim().length === 0}>{busy ? 'Adding…' : 'Add client'}</Button>
		{#if clients.list.length > 0}
			<Button variant="ghost" onclick={() => (adding = false)}>Cancel</Button>
		{/if}
		{#if error}<p id="new-client-error" role="alert" class="w-full text-sm text-attention-ink">{error}</p>{/if}
	</form>
{/snippet}

{#if clients.status === 'loading' || clients.status === 'idle'}
	<div class="mb-6 flex items-center gap-3" aria-busy="true" aria-label="Loading clients">
		<Skeleton class="h-9 w-24 rounded-full" />
		<Skeleton class="h-9 w-56 rounded-lg" />
	</div>
{:else if clients.status === 'error'}
	<div role="alert" class="mb-6 flex flex-wrap items-center gap-3 rounded-xl border border-border bg-card p-4 text-sm">
		<p>Could not load the clients. {clients.error}</p>
		<Button variant="outline" size="sm" onclick={() => loadClients(true)}>Try again</Button>
	</div>
{:else if clients.list.length === 0}
	<Empty.Root class="mb-6 border border-dashed border-border">
		<Empty.Header>
			<Empty.Title>Add your first client</Empty.Title>
			<Empty.Description>Facts, briefs and assets belong to a client: the business the site is for.</Empty.Description>
		</Empty.Header>
		<Empty.Content class="w-full max-w-lg">{@render addForm()}</Empty.Content>
	</Empty.Root>
{:else}
	<div class="mb-6 flex flex-wrap items-end gap-3">
		<div class="grid gap-2">
			<Label for="client">Client</Label>
			<NativeSelect.Root
				id="client"
				class="min-w-56"
				value={clients.selected}
				onchange={(event) => selectClient((event.currentTarget as HTMLSelectElement).value)}
			>
				{#each clients.list as client (client.id)}
					<NativeSelect.Option value={client.id}>{client.name}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
		</div>
		{#if !adding}
			<Button variant="outline" onclick={() => (adding = true)}>Add a client</Button>
		{/if}
	</div>
	{#if adding}
		<div class="mb-6 rounded-xl border border-border bg-card p-4">{@render addForm()}</div>
	{/if}
{/if}
