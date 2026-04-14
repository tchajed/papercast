<script lang="ts">
	import { page } from '$app/stores';
	import { onMount, onDestroy } from 'svelte';
	import {
		getFeed,
		submitEpisode,
		uploadPdf,
		formatDuration,
		type FeedWithEpisodes,
		type Episode,
	} from '$lib/api';

	let feed = $state<FeedWithEpisodes | null>(null);
	let error = $state('');
	let submitUrl = $state('');
	let submitting = $state(false);

	// PDF upload
	let pdfFile = $state<File | null>(null);
	let pdfTitle = $state('');
	let pdfSourceUrl = $state('');
	let uploadingPdf = $state(false);

	// TTS options
	let summarize = $state(false);

	let token = $derived($page.params.token ?? '');
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	async function loadFeed() {
		try {
			feed = await getFeed(token);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load feed';
		}
	}

	function hasInProgress(episodes: Episode[]): boolean {
		return episodes.some((ep) => !['done', 'error'].includes(ep.status));
	}

	function startPolling() {
		stopPolling();
		pollInterval = setInterval(async () => {
			if (feed && hasInProgress(feed.episodes)) {
				await loadFeed();
			}
		}, 5000);
	}

	function stopPolling() {
		if (pollInterval) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
	}

	onMount(async () => {
		await loadFeed();
		startPolling();
	});

	onDestroy(() => stopPolling());

	async function handleSubmitUrl() {
		if (!submitUrl.trim()) return;
		submitting = true;
		error = '';
		try {
			await submitEpisode(token, submitUrl, { summarize });
			submitUrl = '';
			await loadFeed();
			startPolling();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to submit episode';
		} finally {
			submitting = false;
		}
	}

	async function handleUploadPdf() {
		if (!pdfFile) return;
		uploadingPdf = true;
		error = '';
		try {
			await uploadPdf(token, pdfFile, pdfTitle || undefined, {
				summarize,
				sourceUrl: pdfSourceUrl.trim() || undefined,
			});
			pdfFile = null;
			pdfTitle = '';
			pdfSourceUrl = '';
			await loadFeed();
			startPolling();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to upload PDF';
		} finally {
			uploadingPdf = false;
		}
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}

	function badgeClass(status: string): string {
		if (status === 'done') return 'badge done';
		if (status === 'error') return 'badge error';
		if (status === 'pending') return 'badge pending';
		return 'badge processing';
	}

	function handleFileInput(e: Event) {
		const target = e.target as HTMLInputElement;
		pdfFile = target.files?.[0] ?? null;
	}
</script>

{#if feed}
	<p class="mb-2"><a href="/">&larr; All feeds</a></p>

	<!-- Feed info -->
	<div class="card mb-2 feed-info">
		<h2 style="margin-bottom: 0.5rem;">{feed.title}</h2>
		{#if feed.description}
			<p class="muted mb-1">{feed.description}</p>
		{/if}
		<p style="font-size: 0.875rem; margin-bottom: 0.75rem;">
			This feed converts articles and papers to audio using text-to-speech.
			Copy the RSS URL below and add it as a custom feed in your podcast app
			(e.g., in Overcast: Library &rarr; Add URL).
		</p>
		<button class="primary" onclick={() => feed && copyToClipboard(feed.rss_url)}>
			Copy RSS URL
		</button>
	</div>

	<!-- Unified submission form -->
	<div class="card mb-2">
		<form onsubmit={(e) => {
			e.preventDefault();
			if (pdfFile) {
				handleUploadPdf();
			} else {
				handleSubmitUrl();
			}
		}}>
			<h3 class="form-heading">Add a paper</h3>

			<div class="mb-1">
				<input
					bind:value={submitUrl}
					placeholder="Paste a URL (e.g. https://arxiv.org/abs/2301.07041)"
					disabled={submitting || uploadingPdf}
				/>
			</div>

			<div class="mb-1">
				<label class="file-drop" class:has-file={!!pdfFile}>
					<input
						type="file"
						accept=".pdf"
						onchange={handleFileInput}
						disabled={submitting || uploadingPdf}
						class="file-input-hidden"
					/>
					{#if pdfFile}
						<span class="file-drop-text">{pdfFile.name}</span>
						<button
							type="button"
							class="file-remove"
							onclick={(e) => { e.preventDefault(); pdfFile = null; pdfTitle = ''; pdfSourceUrl = ''; }}
						>&times;</button>
					{:else}
						<span class="file-drop-text muted">Or choose a PDF file</span>
					{/if}
				</label>
			</div>

			{#if pdfFile}
				<div class="mb-1">
					<input bind:value={pdfTitle} placeholder="Title (optional)" disabled={uploadingPdf} />
				</div>
				<div class="mb-1">
					<input bind:value={pdfSourceUrl} placeholder="Source URL (optional)" disabled={uploadingPdf} />
				</div>
			{/if}

			<div class="flex-between">
				<label class="checkbox-label">
					<input type="checkbox" bind:checked={summarize} />
					<span>Summarize</span>
				</label>
				<button
					type="submit"
					class="primary"
					disabled={submitting || uploadingPdf || (!submitUrl.trim() && !pdfFile)}
				>
					{#if submitting || uploadingPdf}
						Adding...
					{:else}
						Add
					{/if}
				</button>
			</div>
			<p class="muted" style="font-size: 0.75rem; margin-top: 0.25rem;">
				Summarize condenses the text to ~20-30% before converting to speech.
			</p>
		</form>
	</div>

	{#if error}
		<div class="card mb-2" style="border-color: var(--danger); color: var(--danger);">{error}</div>
	{/if}

	<h2>Episodes</h2>

	{#each feed.episodes as ep}
		<div class="card">
			<div class="flex-between mb-1">
				<div class="flex">
					{#if ep.image_url}
						<img src={ep.image_url} alt="" style="width:40px; height:40px; border-radius:4px; object-fit:cover;" />
					{/if}
					<a href="/feeds/{token}/episodes/{ep.id}">
						<strong>{ep.title}</strong>
					</a>
				</div>
				<span class={badgeClass(ep.status)}>{ep.status}</span>
			</div>
			<div class="muted" style="font-size: 0.8rem;">
				{#if ep.source_url}
					<a href={ep.source_url} target="_blank" rel="noopener">{ep.source_url}</a>
				{:else}
					PDF upload
				{/if}
			</div>
			{#if ep.description}
				<p style="font-size: 0.9rem; margin-top: 0.5rem;">{ep.description}</p>
			{/if}
			{#if ep.retry_at}
				<div style="font-size: 0.8rem; margin-top: 0.5rem; color: #92400e;">
					⏳ Waiting on retry at {new Date(ep.retry_at + 'Z').toLocaleString()}
				</div>
			{/if}
			{#if ep.status === 'error' && ep.error_msg}
				<div style="color: var(--danger); font-size: 0.85rem; margin-top: 0.5rem;">
					{ep.error_msg}
				</div>
			{/if}
			{#if ep.status === 'done' && ep.audio_url}
				<div class="mt-2 flex">
					<audio controls src={ep.audio_url} preload="metadata" style="height: 32px;"></audio>
					<span class="muted">{formatDuration(ep.duration_secs)}</span>
				</div>
			{/if}
		</div>
	{:else}
		<p class="muted">No episodes yet. Submit a URL or upload a PDF above.</p>
	{/each}
{:else if error}
	<div class="card" style="border-color: var(--danger); color: var(--danger);">{error}</div>
{:else}
	<p class="muted">Loading...</p>
{/if}

<style>
	.feed-info h2 {
		margin-bottom: 0.25rem;
	}

	.form-heading {
		font-size: 1rem;
		font-weight: 600;
		margin-bottom: 0.75rem;
	}

	.file-drop {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		border: 1.5px dashed var(--border);
		border-radius: 6px;
		padding: 0.625rem 0.75rem;
		cursor: pointer;
		transition: border-color 0.15s, background 0.15s;
	}

	.file-drop:hover {
		border-color: var(--primary);
		background: rgba(37, 99, 235, 0.03);
	}

	.file-drop.has-file {
		border-style: solid;
		border-color: var(--primary);
		background: rgba(37, 99, 235, 0.04);
	}

	.file-input-hidden {
		display: none;
	}

	.file-drop-text {
		font-size: 0.875rem;
	}

	.file-remove {
		background: none;
		border: none;
		color: var(--text-muted);
		font-size: 1.1rem;
		padding: 0 0.25rem;
		line-height: 1;
	}

	.file-remove:hover {
		color: var(--danger);
	}

	.checkbox-label {
		display: flex;
		align-items: center;
		gap: 0.375rem;
		cursor: pointer;
		font-size: 0.875rem;
		font-weight: 500;
	}
</style>
