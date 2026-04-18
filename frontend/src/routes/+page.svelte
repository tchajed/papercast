<script lang="ts">
	import { onMount } from 'svelte';
	import { listFeeds, createFeed, deleteFeed, updateFeed, regenerateFeedImage, type Feed } from '$lib/api';
	import Toast from '$lib/Toast.svelte';
	import { Plus, Rss, Pencil, ImagePlus, Trash2, Save, X } from 'lucide-svelte';

	const urlToken = new URLSearchParams(window.location.search).get('token');
	if (urlToken) {
		localStorage.setItem('adminToken', urlToken);
		// Remove token from URL without adding to browser history
		const url = new URL(window.location.href);
		url.searchParams.delete('token');
		window.history.replaceState({}, '', url);
	}
	let adminToken = $state(localStorage.getItem('adminToken') ?? '');
	let feeds = $state<Feed[]>([]);
	let error = $state('');
	let loaded = $state(false);
	let showCreate = $state(false);

	// Create form
	let newSlug = $state('');
	let newTitle = $state('');
	let newDescription = $state('');
	let newTts = $state('google');

	async function loadFeeds() {
		if (!adminToken) return;
		try {
			error = '';
			feeds = await listFeeds(adminToken);
			localStorage.setItem('adminToken', adminToken);
			loaded = true;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load feeds';
		}
	}

	async function handleCreate() {
		try {
			error = '';
			await createFeed(adminToken, {
				slug: newSlug,
				title: newTitle,
				description: newDescription,
				tts_default: newTts,
			});
			showCreate = false;
			newSlug = '';
			newTitle = '';
			newDescription = '';
			await loadFeeds();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to create feed';
		}
	}

	let editingToken = $state<string | null>(null);
	let editSlug = $state('');
	let editTitle = $state('');
	let editDescription = $state('');

	function startEdit(feed: Feed) {
		if (!feed.feed_token) return;
		editingToken = feed.feed_token;
		editSlug = feed.slug;
		editTitle = feed.title;
		editDescription = feed.description;
	}

	async function handleEditSave() {
		if (!editingToken) return;
		try {
			error = '';
			await updateFeed(adminToken, editingToken, {
				slug: editSlug,
				title: editTitle,
				description: editDescription,
			});
			editingToken = null;
			await loadFeeds();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to update feed';
		}
	}

	let regenerating = $state<string | null>(null);

	async function handleRegenerateImage(token: string) {
		try {
			error = '';
			regenerating = token;
			await regenerateFeedImage(adminToken, token);
			await loadFeeds();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to regenerate image';
		} finally {
			regenerating = null;
		}
	}

	async function handleDelete(token: string) {
		if (!confirm('Delete this feed and all its episodes?')) return;
		try {
			await deleteFeed(adminToken, token);
			await loadFeeds();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete feed';
		}
	}

	let toastMessage = $state('');

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
		toastMessage = 'RSS URL copied to clipboard';
	}

	onMount(() => {
		if (adminToken) loadFeeds();
	});
</script>

<div>
	{#if !loaded}
		<div class="card bg-base-100 shadow-sm border border-base-300">
			<div class="card-body">
				<label class="label" for="admin-token">Admin Token</label>
				<div class="flex gap-2">
					<input id="admin-token" type="password" class="input input-bordered flex-1" bind:value={adminToken} placeholder="Enter admin token" />
					<button class="btn btn-primary" onclick={loadFeeds}>Load Feeds</button>
				</div>
			</div>
		</div>
	{:else}
		<p class="opacity-60 text-sm mb-4">Convert articles, papers, and other written content to audio using text-to-speech.</p>

		<div class="flex justify-between items-center mb-4">
			<h2 class="text-xl font-semibold">Feeds</h2>
			<button class="btn btn-primary btn-sm" onclick={() => showCreate = !showCreate}>
				{#if showCreate}
					<X size={16} /> Cancel
				{:else}
					<Plus size={16} /> New Feed
				{/if}
			</button>
		</div>

		{#if showCreate}
			<div class="card bg-base-100 shadow-sm border border-base-300 mb-4">
				<div class="card-body">
					<form onsubmit={(e) => { e.preventDefault(); handleCreate(); }}>
						<fieldset class="fieldset">
							<label class="fieldset-label">Slug</label>
							<input class="input input-bordered w-full" bind:value={newSlug} placeholder="ml-papers" required />
						</fieldset>
						<fieldset class="fieldset">
							<label class="fieldset-label">Title</label>
							<input class="input input-bordered w-full" bind:value={newTitle} placeholder="ML Papers" required />
						</fieldset>
						<fieldset class="fieldset">
							<label class="fieldset-label">Description</label>
							<input class="input input-bordered w-full" bind:value={newDescription} placeholder="Optional description" />
						</fieldset>
						<div class="mt-2">
							<button type="submit" class="btn btn-primary btn-sm">Create Feed</button>
						</div>
					</form>
				</div>
			</div>
		{/if}

		{#if error}
			<div role="alert" class="alert alert-error mb-4">{error}</div>
		{/if}

		{#each feeds as feed}
			<div class="card bg-base-100 shadow-sm border border-base-300 mb-3">
				<div class="card-body p-4">
					{#if editingToken === feed.feed_token}
						<form onsubmit={(e) => { e.preventDefault(); handleEditSave(); }}>
							<fieldset class="fieldset">
								<label class="fieldset-label">Slug</label>
								<input class="input input-bordered w-full" bind:value={editSlug} required />
							</fieldset>
							<fieldset class="fieldset">
								<label class="fieldset-label">Title</label>
								<input class="input input-bordered w-full" bind:value={editTitle} required />
							</fieldset>
							<fieldset class="fieldset">
								<label class="fieldset-label">Description</label>
								<input class="input input-bordered w-full" bind:value={editDescription} />
							</fieldset>
							<div class="flex gap-2 mt-2">
								<button type="submit" class="btn btn-primary btn-sm"><Save size={14} /> Save</button>
								<button type="button" class="btn btn-ghost btn-sm" onclick={() => (editingToken = null)}><X size={14} /> Cancel</button>
							</div>
						</form>
					{:else}
						<div class="flex justify-between items-center flex-wrap gap-2">
							<div class="flex items-center gap-3">
								{#if feed.image_url}
									<img src={feed.image_url} alt="" width="48" height="48" class="rounded" />
								{/if}
								<div>
									<a href="/feeds/{feed.feed_token}" class="font-semibold link">{feed.title}</a>
									<span class="text-sm opacity-60">({feed.slug})</span>
								</div>
							</div>
							<div class="flex gap-1 flex-wrap">
								<button class="btn btn-ghost btn-xs" onclick={() => startEdit(feed)}><Pencil size={14} /> Edit</button>
								<button
									class="btn btn-ghost btn-xs"
									onclick={() => feed.feed_token && handleRegenerateImage(feed.feed_token)}
									disabled={regenerating === feed.feed_token}
								>
									<ImagePlus size={14} />
									{regenerating === feed.feed_token ? 'Generating...' : feed.image_url ? 'Regen image' : 'Gen image'}
								</button>
								<button class="btn btn-error btn-xs" onclick={() => feed.feed_token && handleDelete(feed.feed_token)}><Trash2 size={14} /> Delete</button>
							</div>
						</div>
						{#if feed.description}
							<p class="text-sm opacity-60 mt-1">{feed.description}</p>
						{/if}
						<div class="flex items-center gap-2 mt-2 text-xs opacity-60">
							<span>{feed.episode_count ?? 0} episodes</span>
							{#if feed.rss_url}
								<span>&middot;</span>
								<button class="btn btn-ghost btn-xs" onclick={() => feed.rss_url && copyToClipboard(feed.rss_url)}>
									<Rss size={12} /> Copy RSS URL
								</button>
							{/if}
						</div>
					{/if}
				</div>
			</div>
		{:else}
			<p class="opacity-60">No feeds yet. Create one to get started.</p>
		{/each}
	{/if}
</div>

{#if toastMessage}
	<Toast message={toastMessage} onclose={() => toastMessage = ''} />
{/if}
