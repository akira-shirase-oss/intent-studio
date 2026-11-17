<script lang="ts">
	import * as Alert from '@intent-studio/ui/ui/alert';
	import type { Problem } from '#lib/api.ts';
	import { problemText } from '#lib/words.ts';

	// A summary of what was refused, in plain words, each linked to its field when the form has
	// one; the server's codes stay behind a details toggle.
	let {
		title,
		problems,
		label = (path: string) => path,
		fieldId = () => null
	}: {
		title: string;
		problems: Problem[];
		label?: (path: string) => string;
		fieldId?: (path: string) => string | null;
	} = $props();
</script>

<Alert.Root role="alert" class="mb-6">
	<Alert.Title>{title}</Alert.Title>
	<Alert.Description>
		<ul class="mt-2 grid gap-1.5">
			{#each problems as problem, index (index)}
				{@const id = fieldId(problem.path)}
				<li>
					{#if id}<a class="font-medium underline underline-offset-4" href={`#${id}`}>{label(problem.path)}</a>{:else}<span class="font-medium">{label(problem.path)}</span>{/if}:
					{problemText(problem)}
				</li>
			{/each}
		</ul>
		<details class="mt-3 text-xs text-muted-foreground">
			<summary class="cursor-pointer rounded outline-none focus-visible:ring-4 focus-visible:ring-ring/25">Details for whoever runs the server</summary>
			<ul class="mt-1 font-mono">
				{#each problems as problem, index (index)}
					<li>{problem.path || '(record)'}: {problem.code}{problem.detail ? ` (${problem.detail})` : ''}</li>
				{/each}
			</ul>
		</details>
	</Alert.Description>
</Alert.Root>
