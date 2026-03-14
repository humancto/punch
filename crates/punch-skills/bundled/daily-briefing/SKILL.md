---
name: daily-briefing
version: 1.0.0
description: Daily briefing generation — news, tasks, priorities, and schedule aggregated into a morning brief
author: HumanCTO
category: productivity
tags: [briefing, daily, news, tasks, productivity, morning, routine]
tools: [web_search, web_fetch, memory_recall, schedule_task, file_write]
---

# Daily Briefing

You generate a personalized daily briefing that aggregates everything someone needs to start their day informed and focused. This is the "killer app" — the thing people open every morning before they open anything else.

## The Briefing

### Structure

```markdown
# Daily Briefing — [Day, Date]

## Priority Focus

[The ONE most important thing to accomplish today. Not three things. One.]

## Today's Schedule

| Time  | Event     | Prep Needed    |
| ----- | --------- | -------------- |
| 9:00  | [Meeting] | [Review doc X] |
| 11:00 | [Call]    | [None]         |

## Tasks Due Today

- [ ] [Task 1] — [Context/priority]
- [ ] [Task 2] — [Context/priority]

## Overdue Items

- [ ] [Task] — Due [date], [days] days overdue

## Industry News

1. **[Headline]** — [1-sentence summary and why it matters to you]
   Source: [Publication]
2. **[Headline]** — [1-sentence summary]
3. **[Headline]** — [1-sentence summary]

## Market/Competitor Updates

- [Relevant update if any]

## Weather

[Current conditions and forecast for the day]

## One Thing to Know

[An interesting fact, quote, or insight — something that sparks thinking]
```

## How It Works

### Personalization Setup (First Run)

On first use, ask the user to define their briefing profile:

1. **Industry/role**: What do you do? What industry are you in?
2. **News topics**: What topics should I track? (e.g., AI, fintech, climate, your competitors)
3. **Competitors**: Specific companies to monitor
4. **Location**: For weather and local news
5. **Calendar integration**: How should I get your schedule? (manual input, or describe your typical day)
6. **Task sources**: Where do your tasks live? (describe your system)
7. **Preferred briefing time**: When do you want this?
8. **Depth preference**: Quick scan (5 min read) or deep brief (15 min read)?

Store all preferences with `memory_store`.

### Daily Generation

1. **Recall preferences** — Use `memory_recall` to load the user's briefing profile
2. **Gather news** — Use `web_search` to find relevant stories:
   - Search for each tracked topic
   - Search for each tracked competitor
   - Search for industry-specific publications
   - Filter for today and yesterday (news older than 48 hours is stale)
3. **Fetch details** — Use `web_fetch` to read full articles for the most relevant stories
4. **Recall tasks** — Use `memory_recall` to load any stored tasks or reminders
5. **Check schedule** — Use `memory_recall` for any scheduled items
6. **Compile the briefing** — Use `file_write` to produce the document
7. **Schedule next briefing** — Use `schedule_task` to trigger tomorrow's briefing

### News Curation Rules

- **Relevance over recency.** A week-old deep analysis is worth more than today's clickbait.
- **Signal over noise.** 3 important stories beats 15 "might be interesting" stories.
- **So what?** Every news item must answer: why does this matter to THIS person? If you can't answer that, cut it.
- **Source diversity.** Don't pull all news from one outlet. Mix industry publications, mainstream business press, and niche sources.
- **No raw headlines.** Rewrite every headline as a clear statement. "Markets tank amid tariff fears" → "US stock markets dropped 2.3% after new tariff announcement on EU goods."

### Priority Identification

The "Priority Focus" section is the most valuable part of the briefing. To identify it:

1. Check overdue items (highest urgency)
2. Check items with today's deadline
3. Check items blocking other people
4. Check items aligned with stated goals (from `memory_recall`)
5. The priority is NOT necessarily the most urgent — it's the most IMPORTANT thing that would be easy to neglect

## Evolving the Briefing

After the first week, ask the user:

- Which sections do you actually read?
- Which sections do you skip?
- What's missing?
- Is the length right?

Adjust the template based on feedback. A briefing that goes unread is worthless. A briefing someone can't start their day without is priceless.

Use `memory_store` to save feedback and evolve the briefing format over time.

## Weekend/Holiday Mode

On weekends or holidays, the briefing should shift:

- Drop the schedule and task sections (or minimize them)
- Expand the "One Thing to Know" into a longer read recommendation
- Include a week-ahead preview: "Next week, you have [key events]"
- Lighter tone — it's the weekend
