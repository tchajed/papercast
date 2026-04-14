<script lang="ts">
	import { onMount } from 'svelte';
	import { listFeeds, createFeed, deleteFeed, updateFeed, regenerateFeedImage, type Feed } from '$lib/api';
	import Toast from '$lib/Toast.svelte';
	import { Plus, Rss, Pencil, ImagePlus, Trash2, Save, X } from 'lucide-svelte';

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
		<div class="card">
			<label class="mb-1" style="display:block; font-weight:500;">Admin Token</label>
			<div class="flex">
				<input type="password" bind:value={adminToken} placeholder="Enter admin token" />
				<button class="primary" onclick={loadFeeds}>Load Feeds</button>
			</div>
		</div>
	{:else}
		<div class="flex-between mb-2">
			<h2>Feeds</h2>
			<button class="primary flex" style="display: inline-flex;" onclick={() => showCreate = !showCreate}>
				{#if showCreate}
					<X size={16} /> Cancel
				{:else}
					<Plus size={16} /> New Feed
				{/if}
			</button>
		</div>

		{#if showCreate}
			<div class="card mb-2">
				<form onsubmit={(e) => { e.preventDefault(); handleCreate(); }}>
					<div class="mb-1">
						<label>Slug</label>
						<input bind:value={newSlug} placeholder="ml-papers" required />
					</div>
					<div class="mb-1">
						<label>Title</label>
						<input bind:value={newTitle} placeholder="ML Papers" required />
					</div>
					<div class="mb-1">
						<label>Description</label>
						<input bind:value={newDescription} placeholder="Optional description" />
					</div>
					<button type="submit" class="primary">Create Feed</button>
				</form>
			</div>
		{/if}

		{#if error}
			<div class="card" style="border-color: var(--danger); color: var(--danger);">{error}</div>
		{/if}

		{#each feeds as feed}
			<div class="card">
				{#if editingToken === feed.feed_token}
					<form onsubmit={(e) => { e.preventDefault(); handleEditSave(); }}>
						<div class="mb-1">
							<label>Slug</label>
							<input bind:value={editSlug} required />
						</div>
						<div class="mb-1">
							<label>Title</label>
							<input bind:value={editTitle} required />
						</div>
						<div class="mb-1">
							<label>Description</label>
							<input bind:value={editDescription} />
						</div>
						<div class="flex">
							<button type="submit" class="primary flex" style="display: inline-flex;"><Save size={14} /> Save</button>
							<button type="button" class="flex" style="display: inline-flex;" onclick={() => (editingToken = null)}><X size={14} /> Cancel</button>
						</div>
					</form>
				{:else}
					<div class="flex-between">
						<div class="flex" style="align-items: center;">
							{#if feed.image_url}
								<img src={feed.image_url} alt="" width="48" height="48" style="border-radius: 4px;" />
							{/if}
							<div>
								<a href="/feeds/{feed.feed_token}"><strong>{feed.title}</strong></a>
								<span class="muted">({feed.slug})</span>
							</div>
						</div>
						<div class="flex">
							<button class="flex" style="display: inline-flex;" onclick={() => startEdit(feed)}><Pencil size={14} /> Edit</button>
							<button
								class="flex" style="display: inline-flex;"
								onclick={() => feed.feed_token && handleRegenerateImage(feed.feed_token)}
								disabled={regenerating === feed.feed_token}
							>
								<ImagePlus size={14} />
								{regenerating === feed.feed_token ? 'Generating…' : feed.image_url ? 'Regen image' : 'Gen image'}
							</button>
							<button class="danger flex" style="display: inline-flex;" onclick={() => feed.feed_token && handleDelete(feed.feed_token)}><Trash2 size={14} /> Delete</button>
						</div>
					</div>
					{#if feed.description}
						<p class="muted">{feed.description}</p>
					{/if}
					<div class="flex mt-2 muted" style="font-size: 0.8rem;">
						<span>{feed.episode_count ?? 0} episodes</span>
						{#if feed.rss_url}
							<span>&middot;</span>
							<button class="copy-btn flex" style="display: inline-flex;" onclick={() => feed.rss_url && copyToClipboard(feed.rss_url)}>
								<Rss size={12} /> Copy RSS URL
							</button>
						{/if}
					</div>
				{/if}
			</div>
		{:else}
			<p class="muted">No feeds yet. Create one to get started.</p>
		{/each}
	{/if}
</div>

{#if toastMessage}
	<Toast message={toastMessage} onclose={() => toastMessage = ''} />
{/if}
