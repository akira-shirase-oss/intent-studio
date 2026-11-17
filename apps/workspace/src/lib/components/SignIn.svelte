<script lang="ts">
	import { Button } from '@intent-studio/ui/ui/button';
	import * as Card from '@intent-studio/ui/ui/card';
	import { Input } from '@intent-studio/ui/ui/input';
	import { Label } from '@intent-studio/ui/ui/label';
	import { ApiError, api } from '#lib/api.ts';
	import { failureText } from '#lib/words.ts';

	let { onsignedin }: { onsignedin: () => void } = $props();

	let password = $state('');
	let error = $state<string | null>(null);
	let busy = $state(false);

	async function signIn(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			await api.signIn(password);
			password = '';
			onsignedin();
		} catch (failure) {
			error =
				failure instanceof ApiError && failure.status === 401
					? 'That password is not right.'
					: failure instanceof ApiError && failure.code === 'unavailable' && failure.status === 503 && failure.message.includes('not configured')
						? 'Sign-in is not set up on this server yet. Whoever runs it needs to set INTENT_OPERATOR_PASSWORD_HASH.'
						: failureText(failure);
		} finally {
			busy = false;
		}
	}
</script>

<Card.Root class="mx-auto max-w-sm">
	<Card.Header>
		<Card.Title><h2 class="text-inherit">Operator sign-in</h2></Card.Title>
		<Card.Description>The inbox and every client’s records, behind one password.</Card.Description>
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
					aria-invalid={error ? 'true' : undefined}
					aria-describedby={error ? 'sign-in-error' : undefined}
				/>
			</div>
			{#if error}
				<p id="sign-in-error" class="text-sm text-attention-ink" role="alert">{error}</p>
			{/if}
			<Button type="submit" disabled={busy || password.length === 0}>{busy ? 'Signing in…' : 'Sign in'}</Button>
		</form>
	</Card.Content>
</Card.Root>
