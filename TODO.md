# TODOs

- Add a summarization focus feature: some extra text that narrows the focus of the summary. The test case for this is that in the DeepSeekMath paper I want to focus only on GRPO. There are other cases (mostly ML papers) where I only care about the design and not the evaluation.
- Add "(Summary)" to the episode title for summaries.
- Make the language a bit more generic since this isn't just papers but other articles.
- Make sure markdown is supported by the API and file upload, especially to import AI-generated reports.
- Add a read-only management interface to get status of current jobs without arcane SQL commands. TTS chunk progress should be exposed in the frontend.
- Add time remaining estimation. As each stage completes, we get more info for the later stages to make the estimate better (e.g., we need the content length to have any real estimate).
- Add some mechanism for tracking AI costs (e.g., store token usage for each episode separately). In addition to tokens, I want an automated way (from the command line, not the web interface) to check actual dollar costs of each provider.
- Add ps to the docker container
- Fix feed listing on mobile. The icons don't have the same height, and run off the edge of their container.
- Name the app, add a logo
- Fix build warnings in frontend and backend
- Clean up code
- Update documentation
- Add API documentation, primarily for LLM consumption
- The published/created times on the website still don't make sense to me, there's some time zone issue I suspect. Retry times do look correct.
- Add a little audio boop between sections, the way AI Daily Brief does (but without copying their sound effect). Not sure where to get such a sound effect.
- Fix the chapter timings. For Spanner these are definitely incorrect. Probably the text chunking isn't properly following the headings in the first place.
