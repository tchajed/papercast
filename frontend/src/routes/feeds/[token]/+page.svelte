<script lang="ts">
	import { page } from '$app/stores';
	import { onMount, onDestroy } from 'svelte';
	import {
		getFeed,
		submitEpisode,
		uploadPdf,
		formatDuration,
		episodeTitle,
		type FeedWithEpisodes,
		type Episode,
	} from '$lib/api';
	import { VOICES, DEFAULT_VOICE_ID } from '$lib/voices';
	import Toast from '$lib/Toast.svelte';
	import { Rss, Link, FileUp, Plus, FileText, ExternalLink, Clock, AlertCircle, X, Info } from 'lucide-svelte';

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
	let summarizeFocus = $state('');
	let ttsVoice = $state(DEFAULT_VOICE_ID);

	// Toast
	let toastMessage = $state('');

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
			await submitEpisode(token, submitUrl, { summarize, summarizeFocus: summarizeFocus.trim() || undefined, ttsVoice });
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
				summarizeFocus: summarizeFocus.trim() || undefined,
				ttsVoice,
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
		toastMessage = 'RSS URL copied to clipboard';
	}

	function badgeClass(status: string): string {
		if (status === 'done') return 'badge badge-success';
		if (status === 'error') return 'badge badge-error';
		if (status === 'pending') return 'badge badge-ghost';
		return 'badge badge-warning';
	}

	function handleFileInput(e: Event) {
		const target = e.target as HTMLInputElement;
		pdfFile = target.files?.[0] ?? null;
	}
</script>

{#if feed}
	<!-- Breadcrumbs -->
	<div class="breadcrumbs text-sm mb-4">
		<ul>
			<li><a href="/">Home</a></li>
			<li>{feed.title}</li>
		</ul>
	</div>

	<!-- Feed info -->
	<div class="flex gap-5 items-center mb-6">
		{#if feed.image_url}
			<img src={feed.image_url} alt="" class="w-24 h-24 rounded-xl object-cover shrink-0 shadow-md" />
		{/if}
		<div class="min-w-0 flex-1">
			<h2 class="text-2xl font-bold">{feed.title}</h2>
			{#if feed.description}
				<p class="opacity-60 mt-0.5">{feed.description}</p>
			{/if}
			<div class="flex items-center gap-2 mt-2">
				<button class="btn btn-primary btn-sm" onclick={() => feed && copyToClipboard(feed.rss_url)}>
					<Rss size={16} /> Copy RSS URL
				</button>
				<div class="tooltip tooltip-right" data-tip="Copy the RSS URL and add it as a custom feed in your podcast app (e.g. in Overcast: Library → Search → Add URL).">
					<button type="button" class="btn btn-ghost btn-circle btn-xs opacity-50">
						<Info size={16} />
					</button>
				</div>
			</div>
		</div>
	</div>

	<!-- Submission form -->
	<div class="card bg-base-100 shadow-sm border border-base-300 mb-4">
		<div class="card-body p-4">
			<form onsubmit={(e) => {
				e.preventDefault();
				if (pdfFile) {
					handleUploadPdf();
				} else {
					handleSubmitUrl();
				}
			}}>
				<h3 class="font-semibold mb-3 flex items-center gap-1.5"><Plus size={18} /> Add an episode</h3>

				<div class="mb-3">
					<label class="input input-bordered flex items-center gap-2 w-full">
						<Link size={16} class="opacity-50" />
						<input
							type="text"
							class="grow"
							bind:value={submitUrl}
							placeholder="Paste a URL (e.g. https://arxiv.org/abs/2301.07041)"
							disabled={submitting || uploadingPdf}
						/>
					</label>
				</div>

				<div class="mb-3">
					<label class="flex items-center gap-2 border border-dashed border-base-300 rounded-lg p-3 cursor-pointer hover:border-primary transition-colors" class:border-primary={!!pdfFile} class:border-solid={!!pdfFile}>
						<input
							type="file"
							accept=".pdf,.md,.markdown,.txt"
							onchange={handleFileInput}
							disabled={submitting || uploadingPdf}
							class="hidden"
						/>
						{#if pdfFile}
							<FileText size={16} />
							<span class="text-sm flex-1">{pdfFile.name}</span>
							<button
								type="button"
								class="btn btn-ghost btn-xs"
								onclick={(e) => { e.preventDefault(); pdfFile = null; pdfTitle = ''; pdfSourceUrl = ''; }}
							><X size={14} /></button>
						{:else}
							<FileUp size={16} class="opacity-50" />
							<span class="text-sm opacity-50">Or choose a PDF or Markdown file</span>
						{/if}
					</label>
				</div>

				{#if pdfFile}
					<div class="mb-3">
						<input class="input input-bordered w-full" bind:value={pdfTitle} placeholder="Title (optional)" disabled={uploadingPdf} />
					</div>
					<div class="mb-3">
						<input class="input input-bordered w-full" bind:value={pdfSourceUrl} placeholder="Source URL (optional)" disabled={uploadingPdf} />
					</div>
				{/if}

				<div class="flex justify-between items-center gap-3 flex-wrap">
					<div class="flex items-center gap-4">
						<label class="flex items-center gap-2 cursor-pointer text-sm font-medium">
							<input type="checkbox" class="checkbox checkbox-sm" bind:checked={summarize} />
							<span>Summarize</span>
						</label>
						<label class="flex items-center gap-2 text-sm font-medium">
							<span>Voice</span>
							<select class="select select-bordered select-sm" bind:value={ttsVoice} disabled={submitting || uploadingPdf}>
								{#each VOICES as v}
									<option value={v.id}>{v.label}</option>
								{/each}
							</select>
						</label>
					</div>
					<button
						type="submit"
						class="btn btn-primary btn-sm"
						disabled={submitting || uploadingPdf || (!submitUrl.trim() && !pdfFile)}
					>
						{#if submitting || uploadingPdf}
							<span class="loading loading-spinner loading-xs"></span> Adding...
						{:else}
							<Plus size={16} /> Add
						{/if}
					</button>
				</div>
				{#if summarize}
					<div class="mt-2">
						<input
							class="input input-bordered w-full"
							bind:value={summarizeFocus}
							placeholder="Focus (optional): e.g., focus only on GRPO"
							disabled={submitting || uploadingPdf}
						/>
					</div>
				{/if}
				<p class="text-xs opacity-50 mt-1">
					Summarize condenses the text to ~20-30% before converting to speech.
				</p>
			</form>
		</div>
	</div>

	{#if error}
		<div role="alert" class="alert alert-error mb-4">{error}</div>
	{/if}

	<h2 class="text-xl font-semibold mb-3">Episodes</h2>

	{#each feed.episodes as ep}
		<div class="card bg-base-100 shadow-sm border border-base-300 mb-3">
			<div class="card-body p-4">
				<div class="flex justify-between items-center gap-2 mb-1">
					<div class="flex items-center gap-2 min-w-0">
						{#if ep.image_url}
							<img src={ep.image_url} alt="" class="w-10 h-10 rounded object-cover shrink-0" />
						{/if}
						<a href="/feeds/{token}/episodes/{ep.id}" class="font-semibold link">
							{episodeTitle(ep)}
						</a>
					</div>
					<span class="{badgeClass(ep.status)} shrink-0">
						{ep.status}{#if ep.tts_chunks_total > 0 && ep.status !== 'done' && ep.status !== 'error'}&nbsp;· {ep.tts_chunks_done}/{ep.tts_chunks_total}{/if}
					</span>
				</div>
				<div class="flex items-center gap-1.5 text-xs opacity-60">
					{#if ep.source_url}
						<ExternalLink size={14} />
						<a href={ep.source_url} target="_blank" rel="noopener" class="link truncate">{ep.source_url}</a>
					{:else}
						<FileUp size={14} /> PDF upload
					{/if}
				</div>
				{#if ep.description}
					<p class="text-sm mt-2">{ep.description}</p>
				{/if}
				{#if ep.retry_at}
					<div class="flex items-center gap-1.5 text-xs text-warning mt-2">
						<Clock size={14} /> Waiting on retry at {new Date(ep.retry_at + 'Z').toLocaleString()}
					</div>
				{/if}
				{#if ep.status === 'error' && ep.error_msg}
					<div class="flex items-center gap-1.5 text-sm text-error mt-2">
						<AlertCircle size={14} /> {ep.error_msg}
					</div>
				{/if}
				{#if ep.status === 'done' && ep.audio_url}
					<div class="flex items-center gap-2 mt-2">
						<audio controls src={ep.audio_url} preload="metadata" style="height: 32px;"></audio>
						<span class="text-sm opacity-60">{formatDuration(ep.duration_secs)}</span>
					</div>
				{/if}
			</div>
		</div>
	{:else}
		<p class="opacity-60">No episodes yet. Submit a URL or upload a PDF above.</p>
	{/each}
{:else if error}
	<div role="alert" class="alert alert-error">{error}</div>
{:else}
	<p class="opacity-60">Loading...</p>
{/if}

{#if toastMessage}
	<Toast message={toastMessage} onclose={() => toastMessage = ''} />
{/if}
