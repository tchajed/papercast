<script lang="ts">
	import { listFeeds, createFeed, deleteFeed, type Feed } from '$lib/api';

	let adminToken = $state('');
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

	async function handleDelete(token: string) {
		if (!confirm('Delete this feed and all its episodes?')) return;
		try {
			await deleteFeed(adminToken, token);
			await loadFeeds();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete feed';
		}
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}
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
			<button class="primary" onclick={() => showCreate = !showCreate}>
				{showCreate ? 'Cancel' : 'New Feed'}
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
				<div class="flex-between">
					<div>
						<a href="/feeds/{feed.feed_token}"><strong>{feed.title}</strong></a>
						<span class="muted">({feed.slug})</span>
					</div>
					<button class="danger" onclick={() => feed.feed_token && handleDelete(feed.feed_token)}>Delete</button>
				</div>
				{#if feed.description}
					<p class="muted">{feed.description}</p>
				{/if}
				<div class="flex mt-2 muted" style="font-size: 0.8rem;">
					<span>{feed.episode_count ?? 0} episodes</span>
					{#if feed.rss_url}
						<span>&middot;</span>
						<button class="copy-btn" onclick={() => feed.rss_url && copyToClipboard(feed.rss_url)}>
							Copy RSS URL
						</button>
					{/if}
				</div>
			</div>
		{:else}
			<p class="muted">No feeds yet. Create one to get started.</p>
		{/each}
	{/if}
</div>
