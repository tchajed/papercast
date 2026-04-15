const API_BASE = import.meta.env.VITE_API_BASE_URL || '';

export interface Feed {
	id: string;
	slug: string;
	title: string;
	description: string;
	feed_token?: string;
	tts_default: string;
	rss_url?: string;
	created_at: string;
	episode_count?: number;
	image_url?: string | null;
}

export interface FeedWithEpisodes {
	id: string;
	slug: string;
	title: string;
	description: string;
	tts_default: string;
	rss_url: string;
	image_url?: string | null;
	episodes: Episode[];
}

export interface Episode {
	id: string;
	title: string;
	source_url: string | null;
	source_type: string;
	status: string;
	audio_url: string | null;
	image_url: string | null;
	duration_secs: number | null;
	tts_provider: string | null;
	description: string | null;
	error_msg: string | null;
	pub_date: string | null;
	created_at: string;
	summarize: number;
	retry_at: string | null;
	tts_chunks_done: number;
	tts_chunks_total: number;
}

export interface SubmitEpisodeResponse {
	id: string;
	status: string;
	source_url: string | null;
	source_type: string;
}

async function apiFetch<T>(path: string, options: RequestInit = {}): Promise<T> {
	const headers: Record<string, string> = {
		...((options.headers as Record<string, string>) || {}),
	};
	// Don't set Content-Type for FormData (browser sets boundary automatically)
	if (!(options.body instanceof FormData)) {
		headers['Content-Type'] = 'application/json';
	}

	const resp = await fetch(`${API_BASE}${path}`, {
		...options,
		headers,
	});
	if (!resp.ok) {
		const body = await resp.text();
		throw new Error(`API error ${resp.status}: ${body}`);
	}
	return resp.json();
}

function adminHeaders(adminToken: string): Record<string, string> {
	return { Authorization: `Bearer ${adminToken}` };
}

// Admin endpoints
export async function listFeeds(adminToken: string): Promise<Feed[]> {
	return apiFetch('/api/v1/feeds', { headers: adminHeaders(adminToken) });
}

export async function createFeed(
	adminToken: string,
	data: { slug: string; title: string; description?: string; tts_default?: string }
): Promise<Feed> {
	return apiFetch('/api/v1/feeds', {
		method: 'POST',
		headers: adminHeaders(adminToken),
		body: JSON.stringify(data),
	});
}

export async function updateFeed(
	adminToken: string,
	feedToken: string,
	data: { slug?: string; title?: string; description?: string }
): Promise<Feed> {
	return apiFetch(`/api/v1/feeds/${feedToken}`, {
		method: 'PATCH',
		headers: adminHeaders(adminToken),
		body: JSON.stringify(data),
	});
}

export async function regenerateFeedImage(
	adminToken: string,
	feedToken: string
): Promise<Feed> {
	return apiFetch(`/api/v1/feeds/${feedToken}/image`, {
		method: 'POST',
		headers: adminHeaders(adminToken),
	});
}

export async function deleteFeed(adminToken: string, feedToken: string): Promise<void> {
	await fetch(`${API_BASE}/api/v1/feeds/${feedToken}`, {
		method: 'DELETE',
		headers: adminHeaders(adminToken),
	});
}

// Feed-scoped endpoints (token is the auth)
export async function getFeed(feedToken: string): Promise<FeedWithEpisodes> {
	return apiFetch(`/api/v1/feeds/${feedToken}`);
}

export async function submitEpisode(
	feedToken: string,
	url: string,
	options?: { summarize?: boolean; summarizeFocus?: string }
): Promise<SubmitEpisodeResponse> {
	return apiFetch(`/api/v1/feeds/${feedToken}/episodes`, {
		method: 'POST',
		body: JSON.stringify({
			url,
			summarize: options?.summarize ?? false,
			summarize_focus: options?.summarizeFocus,
		}),
	});
}

export async function uploadPdf(
	feedToken: string,
	file: File,
	title?: string,
	options?: { summarize?: boolean; sourceUrl?: string; summarizeFocus?: string }
): Promise<SubmitEpisodeResponse> {
	const formData = new FormData();
	formData.append('file', file);
	if (title) formData.append('title', title);
	if (options?.summarize) formData.append('summarize', 'true');
	if (options?.sourceUrl) formData.append('source_url', options.sourceUrl);
	if (options?.summarizeFocus) formData.append('summarize_focus', options.summarizeFocus);

	return apiFetch(`/api/v1/feeds/${feedToken}/episodes/pdf`, {
		method: 'POST',
		body: formData,
	});
}

export async function getEpisode(feedToken: string, episodeId: string): Promise<Episode> {
	return apiFetch(`/api/v1/feeds/${feedToken}/episodes/${episodeId}`);
}

export async function updateEpisode(
	feedToken: string,
	episodeId: string,
	data: { title?: string; source_url?: string }
): Promise<Episode> {
	return apiFetch(`/api/v1/feeds/${feedToken}/episodes/${episodeId}`, {
		method: 'PATCH',
		body: JSON.stringify(data),
	});
}

export async function deleteEpisode(feedToken: string, episodeId: string): Promise<void> {
	await fetch(`${API_BASE}/api/v1/feeds/${feedToken}/episodes/${episodeId}`, {
		method: 'DELETE',
	});
}

export interface Section {
	title: string;
	start_secs: number;
}

export async function getEpisodeText(
	feedToken: string,
	episodeId: string
): Promise<{
	cleaned_text: string | null;
	transcript: string | null;
	raw_text: string | null;
	sections: Section[] | null;
}> {
	return apiFetch(`/api/v1/feeds/${feedToken}/episodes/${episodeId}/text`);
}

export function formatTimestamp(secs: number, useHours: boolean): string {
	const total = Math.max(0, Math.floor(secs));
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	const pad = (n: number) => n.toString().padStart(2, '0');
	return useHours ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export async function retryEpisode(feedToken: string, episodeId: string): Promise<unknown> {
	return apiFetch(`/api/v1/feeds/${feedToken}/episodes/${episodeId}/retry`, {
		method: 'POST',
	});
}

export function episodeTitle(ep: { title: string; summarize: number }): string {
	return ep.summarize ? `${ep.title} (Summary)` : ep.title;
}

export function formatDuration(secs: number | null): string {
	if (!secs) return '--:--';
	const m = Math.floor(secs / 60);
	const s = secs % 60;
	return `${m}:${s.toString().padStart(2, '0')}`;
}
