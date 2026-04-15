<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount, onDestroy } from 'svelte';
	import { getEpisode, getEpisodeText, retryEpisode, deleteEpisode, updateEpisode, formatDuration, formatTimestamp, episodeTitle, type Episode, type Section } from '$lib/api';
	import TextModal from '$lib/TextModal.svelte';
	import { ArrowLeft, ExternalLink, FileUp, Clock, AlertCircle, RotateCcw, FileText, ScrollText, Trash2, Pencil } from 'lucide-svelte';

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
			// silently ignore — ToC is optional
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
		if (status === 'done') return 'badge done';
		if (status === 'error') return 'badge error';
		if (status === 'pending') return 'badge pending';
		return 'badge processing';
	}
</script>

<p class="mb-2"><a href="/feeds/{token}" class="flex" style="display: inline-flex; gap: 0.25rem;"><ArrowLeft size={16} /> Back to feed</a></p>

{#if episode}
	<div class="card">
		{#if editing}
			<form onsubmit={(e) => { e.preventDefault(); saveEdit(); }} class="mb-2" style="display: flex; flex-direction: column; gap: 0.5rem;">
				<label style="display: flex; flex-direction: column; gap: 0.25rem;">
					<span class="muted" style="font-size: 0.875rem;">Title</span>
					<input type="text" bind:value={editTitle} required />
				</label>
				<label style="display: flex; flex-direction: column; gap: 0.25rem;">
					<span class="muted" style="font-size: 0.875rem;">Source URL</span>
					<input type="url" bind:value={editSourceUrl} placeholder="(leave blank to clear)" />
				</label>
				<div class="flex" style="gap: 0.5rem;">
					<button type="submit" class="primary" disabled={saving}>{saving ? 'Saving...' : 'Save'}</button>
					<button type="button" onclick={() => (editing = false)} disabled={saving}>Cancel</button>
				</div>
			</form>
		{/if}
		<div class="flex-between mb-1">
			<h2>{episodeTitle(episode)}</h2>
			<span class={badgeClass(episode.status)}>
				{episode.status}{#if episode.tts_chunks_total > 0 && episode.status !== 'done' && episode.status !== 'error'}&nbsp;· {episode.tts_chunks_done}/{episode.tts_chunks_total}{/if}
			</span>
		</div>

		{#if episode.image_url}
			<div class="mb-2">
				<img src={episode.image_url} alt="Episode cover" style="max-width: 200px; border-radius: 8px;" />
			</div>
		{/if}

		<table style="width: 100%; font-size: 0.875rem;"><tbody>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0; white-space: nowrap;">Source</td>
				<td>
					{#if episode.source_url}
						<a href={episode.source_url} target="_blank" rel="noopener">{episode.source_url}</a>
					{:else}
						PDF upload
					{/if}
				</td>
			</tr>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0;">Type</td>
				<td>{episode.source_type}</td>
			</tr>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0;">Summarized</td>
				<td>{episode.summarize ? 'yes' : 'no'}</td>
			</tr>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0;">TTS Provider</td>
				<td>{episode.tts_provider ?? '—'}</td>
			</tr>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0;">Duration</td>
				<td>{formatDuration(episode.duration_secs)}</td>
			</tr>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0;">Published</td>
				<td>{episode.pub_date ? new Date(episode.pub_date + 'Z').toLocaleString() : '—'}</td>
			</tr>
			<tr>
				<td class="muted" style="padding: 0.25rem 1rem 0.25rem 0;">Created</td>
				<td>{new Date(episode.created_at + 'Z').toLocaleString()}</td>
			</tr>
		</tbody></table>

		{#if episode.retry_at}
			<div class="mt-2 flex" style="background: #fef3c7; padding: 0.75rem; border-radius: 6px; font-size: 0.875rem;">
				<Clock size={16} />
				Waiting on retry at {new Date(episode.retry_at + 'Z').toLocaleString()}
				(upstream service unavailable).
			</div>
		{/if}

		{#if episode.status === 'error' && episode.error_msg}
			<div class="mt-2 flex" style="color: var(--danger); background: #fee2e2; padding: 0.75rem; border-radius: 6px;">
				<AlertCircle size={16} />
				<strong>Error:</strong> {episode.error_msg}
			</div>
			<div class="mt-2">
				<button class="primary flex" style="display: inline-flex;" onclick={handleRetry} disabled={retrying}>
					<RotateCcw size={14} />
					{retrying ? 'Retrying...' : 'Retry'}
				</button>
			</div>
		{/if}

		{#if episode.status === 'done' && episode.audio_url}
			<div class="mt-2">
				<audio bind:this={audioEl} controls src={episode.audio_url} style="width: 100%;" preload="metadata"></audio>
			</div>
		{/if}

		{#if episode.description}
			<p class="mt-2" style="white-space: pre-wrap;">{episode.description}</p>
		{/if}

		{#if sections && sections.length > 0}
			<div class="mt-2">
				<h3 style="font-size: 1rem; margin-bottom: 0.5rem;">Chapters</h3>
				<ul style="list-style: none; padding: 0; margin: 0;">
					{#each sections as section}
						<li style="padding: 0.125rem 0;">
							<button
								type="button"
								onclick={() => seekTo(section.start_secs)}
								style="background: none; border: none; padding: 0; color: var(--link, #2563eb); cursor: pointer; font: inherit; text-align: left;"
							>
								<span style="font-variant-numeric: tabular-nums; color: var(--muted, #6b7280); margin-right: 0.5rem;">{formatTimestamp(section.start_secs, useHours)}</span>
								<span>{section.title}</span>
							</button>
						</li>
					{/each}
				</ul>
			</div>
		{/if}

		<div class="mt-2 flex" style="gap: 0.5rem; flex-wrap: wrap;">
			<button class="flex" style="display: inline-flex;" onclick={() => openText('cleaned')} disabled={loadingText}>
				<ScrollText size={14} />
				{episode.summarize ? 'Full text' : 'Transcript'}
			</button>
			{#if episode.summarize}
				<button class="flex" style="display: inline-flex;" onclick={() => openText('transcript')} disabled={loadingText}>
					<FileText size={14} /> Transcript
				</button>
			{/if}
			{#if loadingText}
				<span class="muted">Loading...</span>
			{/if}
			<button class="flex" style="display: inline-flex;" onclick={startEdit} disabled={editing}>
				<Pencil size={14} /> Edit
			</button>
			<button class="danger flex" style="display: inline-flex; margin-left: auto;" onclick={handleDelete} disabled={deleting}>
				<Trash2 size={14} />
				{deleting ? 'Deleting...' : 'Delete episode'}
			</button>
		</div>
	</div>
{:else if error}
	<div class="card" style="border-color: var(--danger); color: var(--danger);">{error}</div>
{:else}
	<p class="muted">Loading...</p>
{/if}

{#if showText === 'cleaned' && cleanedText}
	<TextModal title={episode?.summarize ? 'Full Text' : 'Transcript'} text={cleanedText} onclose={() => showText = false} />
{:else if showText === 'transcript' && transcript}
	<TextModal title="Transcript" text={transcript} onclose={() => showText = false} />
{/if}
