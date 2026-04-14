<script lang="ts">
	import { onMount, onDestroy } from 'svelte';

	let { title, text, onclose }: { title: string; text: string; onclose: () => void } = $props();

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') onclose();
	}

	function onBackdropClick(e: MouseEvent) {
		if (e.target === e.currentTarget) onclose();
	}

	onMount(() => {
		document.body.style.overflow = 'hidden';
	});

	onDestroy(() => {
		document.body.style.overflow = '';
	});
</script>

<svelte:window onkeydown={onKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="modal-backdrop" role="dialog" aria-modal="true" aria-label={title} tabindex="-1" onclick={onBackdropClick}>
	<div class="modal-content">
		<div class="modal-header">
			<h2 style="margin-bottom: 0;">{title}</h2>
			<button class="close-btn" onclick={onclose}>&times;</button>
		</div>
		<div class="modal-body">
			{text}
		</div>
	</div>
</div>

<style>
	.modal-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
		padding: 2rem;
	}

	.modal-content {
		background: var(--surface);
		border-radius: 12px;
		width: 100%;
		max-width: 720px;
		max-height: 80vh;
		display: flex;
		flex-direction: column;
		box-shadow: 0 20px 60px rgba(0, 0, 0, 0.15);
	}

	.modal-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 1rem 1.25rem;
		border-bottom: 1px solid var(--border);
		flex-shrink: 0;
	}

	.close-btn {
		background: none;
		border: none;
		font-size: 1.5rem;
		color: var(--text-muted);
		padding: 0.25rem 0.5rem;
		line-height: 1;
		border-radius: 6px;
	}

	.close-btn:hover {
		background: var(--border);
		color: var(--text);
	}

	.modal-body {
		padding: 1.25rem;
		overflow-y: auto;
		white-space: pre-wrap;
		font-size: 0.9rem;
		line-height: 1.7;
	}
</style>
