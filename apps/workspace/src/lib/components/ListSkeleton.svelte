<script lang="ts">
	import { Skeleton } from '@intent-studio/ui/ui/skeleton';

	let { rows = 3, label }: { rows?: number; label: string } = $props();
</script>

<!-- Same height as a loaded row, so nothing jumps when the list arrives. -->
<ul class="grid gap-3" aria-busy="true" aria-label={label}>
	{#each Array.from({ length: rows }, (_, i) => i) as row (row)}
		<li class="rounded-xl border border-border bg-card p-4">
			<Skeleton class="h-4 w-40" />
			<Skeleton class="mt-3 h-4 w-full max-w-md" />
			<Skeleton class="mt-2 h-4 w-2/3 max-w-sm" />
		</li>
	{/each}
</ul>
