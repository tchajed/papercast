// Keep in sync with SUPPORTED_VOICES in backend/server/src/routes/episodes.rs.
// Premium tier (Studio voices, ~$160/1M chars) will be added here when enabled.
export interface Voice {
	id: string;
	label: string;
	gender: 'male' | 'female';
}

export const VOICES: Voice[] = [
	{ id: 'en-US-Chirp3-HD-Puck', label: 'Puck (male)', gender: 'male' },
	{ id: 'en-US-Chirp3-HD-Kore', label: 'Kore (female)', gender: 'female' },
];

export const DEFAULT_VOICE_ID = 'en-US-Chirp3-HD-Puck';
