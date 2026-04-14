<script lang="ts">
	let { message, onclose }: { message: string; onclose: () => void } = $props();

	$effect(() => {
		const timer = setTimeout(onclose, 2000);
		return () => clearTimeout(timer);
	});
</script>

<div class="toast">
	{message}
</div>

<style>
	.toast {
		position: fixed;
		bottom: 1.5rem;
		left: 50%;
		transform: translateX(-50%);
		background: var(--text);
		color: var(--surface);
		padding: 0.5rem 1.25rem;
		border-radius: 8px;
		font-size: 0.875rem;
		font-weight: 500;
		box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
		z-index: 2000;
		animation: toast-in 0.2s ease-out;
	}

	@keyframes toast-in {
		from {
			opacity: 0;
			transform: translateX(-50%) translateY(0.5rem);
		}
		to {
			opacity: 1;
			transform: translateX(-50%) translateY(0);
		}
	}
</style>
