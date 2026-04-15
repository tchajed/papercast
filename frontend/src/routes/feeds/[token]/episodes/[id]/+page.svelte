<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount, onDestroy } from 'svelte';
	import { getEpisode, getEpisodeText, retryEpisode, deleteEpisode, updateEpisode, formatDuration, formatTimestamp, episodeTitle, type Episode, type Section } from '$lib/api';
	import TextModal from '$lib/TextModal.svelte';
	import { ExternalLink, FileUp, Clock, AlertCircle, RotateCcw, FileText, ScrollText, Trash2, Pencil } from 'lucide-svelte';

	let episode = $state<Episode | null>(null);
	let error = $state('');
	let retrying = $state(false);
	let deleting = $state(false);
	let editing = $state(false);
	let editTitle = $state('');
	let editSourceUrl = $state('');
	let saving = $state(false);
	let showText = $state<false | 'cleaned' | 'transcript'>(false);
	let cleanedText = $state<string | null>(null);
	let transcript = $state<string | null>(null);
	let sections = $state<Section[] | null>(null);
	let loadingText = $state(false);
	let audioEl = $state<HTMLAudioElement | null>(null);
	let feedTitle = $state('');

	let useHours = $derived(
		(episode?.duration_secs ?? 0) >= 3600 ||
			(sections?.[sections.length - 1]?.start_secs ?? 0) >= 3600
	);

	async function loadSections() {
		if (sections !== null) return;
		try {
			const data = await getEpisodeText(token, episodeId);
			cleanedText = data.cleaned_text;
			transcript = data.transcript ?? null;
			sections = data.sections ?? [];
		} catch {
			sections = [];
		}
	}

	function seekTo(secs: number) {
		if (!audioEl) return;
		audioEl.currentTime = secs;
		audioEl.play().catch(() => {});
	}

	let token = $derived($page.params.token ?? '');
	let episodeId = $derived($page.params.id ?? '');
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	async function loadEpisode() {
		try {
			episode = await getEpisode(token, episodeId);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load episode';
		}
	}

	onMount(async () => {
		// Load feed title for breadcrumbs
		try {
			const { getFeed } = await import('$lib/api');
			const feed = await getFeed(token);
			feedTitle = feed.title;
		} catch {
			feedTitle = 'Feed';
		}

		await loadEpisode();
		if (episode?.status === 'done') {
			loadSections();
		}
		pollInterval = setInterval(async () => {
			if (episode && !['done', 'error'].includes(episode.status)) {
				await loadEpisode();
				if (episode?.status === 'done') {
					loadSections();
				}
			}
		}, 5000);
	});

	onDestroy(() => {
		if (pollInterval) clearInterval(pollInterval);
	});

	async function handleDelete() {
		if (!confirm('Delete this episode? This removes the audio and image from storage and cannot be undone.')) {
			return;
		}
		deleting = true;
		try {
			await deleteEpisode(token, episodeId);
			goto(`/feeds/${token}`);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete';
			deleting = false;
		}
	}

	function startEdit() {
		if (!episode) return;
		editTitle = episode.title;
		editSourceUrl = episode.source_url ?? '';
		editing = true;
	}

	async function saveEdit() {
		if (!episode) return;
		saving = true;
		try {
			const updated = await updateEpisode(token, episodeId, {
				title: editTitle,
				source_url: editSourceUrl,
			});
			episode = updated;
			editing = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to update';
		} finally {
			saving = false;
		}
	}

	async function handleRetry() {
		retrying = true;
		try {
			await retryEpisode(token, episodeId);
			await loadEpisode();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to retry';
		} finally {
			retrying = false;
		}
	}

	async function loadTextData() {
		if (cleanedText !== null) return;
		loadingText = true;
		try {
			const data = await getEpisodeText(token, episodeId);
			cleanedText = data.cleaned_text;
			transcript = data.transcript ?? null;
			sections = data.sections ?? [];
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load text';
		} finally {
			loadingText = false;
		}
	}

	async function openText(which: 'cleaned' | 'transcript') {
		await loadTextData();
		showText = which;
	}

	function badgeClass(status: string): string {
		if (status === 'done') return 'badge badge-success';
		if (status === 'error') return 'badge badge-error';
		if (status === 'pending') return 'badge badge-ghost';
		return 'badge badge-warning';
	}
</script>

<!-- Breadcrumbs -->
<div class="breadcrumbs text-sm mb-4">
	<ul>
		<li><a href="/">Home</a></li>
		<li><a href="/feeds/{token}">{feedTitle || 'Feed'}</a></li>
		<li>{episode ? episodeTitle(episode) : 'Episode'}</li>
	</ul>
</div>

{#if episode}
	<div class="card bg-base-100 shadow-sm border border-base-300">
		<div class="card-body p-4">
			{#if editing}
				<form onsubmit={(e) => { e.preventDefault(); saveEdit(); }} class="mb-4">
					<fieldset class="fieldset">
						<label class="fieldset-label">Title</label>
						<input type="text" class="input input-bordered w-full" bind:value={editTitle} required />
					</fieldset>
					<fieldset class="fieldset">
						<label class="fieldset-label">Source URL</label>
						<input type="url" class="input input-bordered w-full" bind:value={editSourceUrl} placeholder="(leave blank to clear)" />
					</fieldset>
					<div class="flex gap-2 mt-2">
						<button type="submit" class="btn btn-primary btn-sm" disabled={saving}>{saving ? 'Saving...' : 'Save'}</button>
						<button type="button" class="btn btn-ghost btn-sm" onclick={() => (editing = false)} disabled={saving}>Cancel</button>
					</div>
				</form>
			{/if}

			<div class="flex justify-between items-center gap-2 mb-2">
				<h2 class="text-xl font-semibold min-w-0">{episodeTitle(episode)}</h2>
				<span class="{badgeClass(episode.status)} shrink-0">
					{episode.status}{#if episode.tts_chunks_total > 0 && episode.status !== 'done' && episode.status !== 'error'}&nbsp;· {episode.tts_chunks_done}/{episode.tts_chunks_total}{/if}
				</span>
			</div>

			{#if episode.image_url}
				<div class="mb-4">
					<img src={episode.image_url} alt="Episode cover" class="max-w-48 rounded-lg" />
				</div>
			{/if}

			<div class="overflow-x-auto">
				<table class="table table-sm">
					<tbody>
						<tr><td class="opacity-60 w-32">Source</td><td>
							{#if episode.source_url}
								<a href={episode.source_url} target="_blank" rel="noopener" class="link">{episode.source_url}</a>
							{:else}
								PDF upload
							{/if}
						</td></tr>
						<tr><td class="opacity-60">Type</td><td>{episode.source_type}</td></tr>
						<tr><td class="opacity-60">Summarized</td><td>{episode.summarize ? 'yes' : 'no'}</td></tr>
						<tr><td class="opacity-60">TTS Provider</td><td>{episode.tts_provider ?? '\u2014'}</td></tr>
						<tr><td class="opacity-60">Duration</td><td>{formatDuration(episode.duration_secs)}</td></tr>
						<tr><td class="opacity-60">Published</td><td>{episode.pub_date ? new Date(episode.pub_date + 'Z').toLocaleString() : '\u2014'}</td></tr>
						<tr><td class="opacity-60">Created</td><td>{new Date(episode.created_at + 'Z').toLocaleString()}</td></tr>
					</tbody>
				</table>
			</div>

			{#if episode.retry_at}
				<div role="alert" class="alert alert-warning mt-4">
					<Clock size={16} />
					<span>Waiting on retry at {new Date(episode.retry_at + 'Z').toLocaleString()} (upstream service unavailable).</span>
				</div>
			{/if}

			{#if episode.status === 'error' && episode.error_msg}
				<div role="alert" class="alert alert-error mt-4">
					<AlertCircle size={16} />
					<span><strong>Error:</strong> {episode.error_msg}</span>
				</div>
				<div class="mt-3">
					<button class="btn btn-primary btn-sm" onclick={handleRetry} disabled={retrying}>
						<RotateCcw size={14} />
						{retrying ? 'Retrying...' : 'Retry'}
					</button>
				</div>
			{/if}

			{#if episode.status === 'done' && episode.audio_url}
				<div class="mt-4">
					<audio bind:this={audioEl} controls src={episode.audio_url} class="w-full" preload="metadata"></audio>
				</div>
			{/if}

			{#if episode.description}
				<p class="mt-4 whitespace-pre-wrap">{episode.description}</p>
			{/if}

			{#if sections && sections.length > 0}
				<div class="mt-4">
					<h3 class="font-semibold mb-2">Chapters</h3>
					<ul class="menu menu-sm bg-base-200 rounded-lg w-full">
						{#each sections as section}
							<li>
								<button type="button" onclick={() => seekTo(section.start_secs)}>
									<span class="tabular-nums opacity-60 w-16 text-right shrink-0">{formatTimestamp(section.start_secs, useHours)}</span>
									<span>{section.title}</span>
								</button>
							</li>
						{/each}
					</ul>
				</div>
			{/if}

			<div class="flex flex-wrap items-center gap-2 mt-4">
				<button class="btn btn-ghost btn-sm" onclick={() => openText('cleaned')} disabled={loadingText}>
					<ScrollText size={14} />
					{episode.summarize ? 'Full text' : 'Transcript'}
				</button>
				{#if episode.summarize}
					<button class="btn btn-ghost btn-sm" onclick={() => openText('transcript')} disabled={loadingText}>
						<FileText size={14} /> Transcript
					</button>
				{/if}
				{#if loadingText}
					<span class="loading loading-spinner loading-xs"></span>
				{/if}
				<button class="btn btn-ghost btn-sm" onclick={startEdit} disabled={editing}>
					<Pencil size={14} /> Edit
				</button>
				<div class="flex-1"></div>
				<button class="btn btn-error btn-sm" onclick={handleDelete} disabled={deleting}>
					<Trash2 size={14} />
					{deleting ? 'Deleting...' : 'Delete episode'}
				</button>
			</div>
		</div>
	</div>
{:else if error}
	<div role="alert" class="alert alert-error">{error}</div>
{:else}
	<p class="opacity-60">Loading...</p>
{/if}

{#if showText === 'cleaned' && cleanedText}
	<TextModal title={episode?.summarize ? 'Full Text' : 'Transcript'} text={cleanedText} onclose={() => showText = false} />
{:else if showText === 'transcript' && transcript}
	<TextModal title="Transcript" text={transcript} onclose={() => showText = false} />
{/if}
