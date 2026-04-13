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
			await uploadPdf(token, pdfFile, pdfTitle || undefined, { summarize });
			pdfFile = null;
			pdfTitle = '';
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

	<div class="flex-between mb-2">
		<div>
			<h2>{feed.title}</h2>
			<p class="muted">{feed.description}</p>
		</div>
		<button class="copy-btn" onclick={() => feed && copyToClipboard(feed.rss_url)}>
			Copy RSS URL
		</button>
	</div>

	<!-- TTS options -->
	<div class="card mb-2">
		<label style="display:flex; align-items:center; gap:0.5rem; cursor:pointer;">
			<input type="checkbox" bind:checked={summarize} />
			<span style="font-weight:500;">Summarize before TTS</span>
		</label>
		<p class="muted" style="font-size: 0.8rem; margin-top: 0.25rem;">
			Condenses the text to ~20-30% before converting to speech.
		</p>
	</div>

	<!-- URL submission -->
	<div class="card mb-2">
		<form onsubmit={(e) => { e.preventDefault(); handleSubmitUrl(); }}>
			<label class="mb-1" style="display:block; font-weight:500;">Submit URL</label>
			<div class="flex">
				<input
					bind:value={submitUrl}
					placeholder="https://arxiv.org/abs/2301.07041 or article URL"
					disabled={submitting}
				/>
				<button type="submit" class="primary" disabled={submitting}>
					{submitting ? 'Submitting...' : 'Submit'}
				</button>
			</div>
		</form>
	</div>

	<!-- PDF upload -->
	<div class="card mb-2">
		<form onsubmit={(e) => { e.preventDefault(); handleUploadPdf(); }}>
			<label class="mb-1" style="display:block; font-weight:500;">Upload PDF</label>
			<div class="flex mb-1">
				<input type="file" accept=".pdf" onchange={handleFileInput} disabled={uploadingPdf} />
			</div>
			<div class="flex">
				<input bind:value={pdfTitle} placeholder="Title (optional)" disabled={uploadingPdf} />
				<button type="submit" class="primary" disabled={uploadingPdf || !pdfFile}>
					{uploadingPdf ? 'Uploading...' : 'Upload'}
				</button>
			</div>
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
			{#if ep.status === 'error' && ep.error_msg}
				<div style="color: var(--danger); font-size: 0.85rem; margin-top: 0.5rem;">
					{ep.error_msg}
				</div>
			{/if}
			{#if ep.status === 'done' && ep.audio_url}
				<div class="mt-2 flex">
					<audio controls src={ep.audio_url} preload="none" style="height: 32px;"></audio>
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
