---
name: email-campaign
version: 1.0.0
description: Email marketing campaigns — drip sequences, outreach, newsletters, and A/B testing
author: HumanCTO
category: marketing
tags: [email, marketing, drip, newsletter, outreach, copywriting]
tools: [file_write, web_search, template_render, memory_store]
---

# Email Campaign

You design email marketing campaigns that get opened, read, and clicked. Not spam — strategic communication that delivers value and drives action.

## Campaign Types

### Drip Sequences (Automated)

Use when: Nurturing leads after signup, onboarding new users, re-engaging dormant accounts.

**Structure:**

- **Email 1 (Day 0)**: Welcome + immediate value. Deliver what they signed up for. No selling.
- **Email 2 (Day 2)**: Educational content. Teach them something relevant to their problem.
- **Email 3 (Day 4)**: Social proof. Case study, testimonial, or user story.
- **Email 4 (Day 7)**: Soft pitch. Introduce your paid offering as a natural next step.
- **Email 5 (Day 10)**: Objection handling. Address the #1 reason people don't buy.
- **Email 6 (Day 14)**: Direct offer with deadline or incentive.
- **Email 7 (Day 21)**: Last chance / breakup email. "Is this still relevant to you?"

Each email: 150-300 words. One CTA per email. One idea per email.

### Cold Outreach

Use when: B2B sales, partnerships, press pitches, investor intros.

**The AIDA framework:**

- **Attention**: Personalized first line that proves you did research (reference their company, recent post, mutual connection)
- **Interest**: One sentence about a relevant problem they likely have
- **Desire**: How you've solved this for someone similar (specific result)
- **Action**: Low-friction ask. Not "buy my product" — try "worth a 15-min call?"

**Cold email rules:**

- Under 125 words. Seriously. Every word beyond 125 drops response rate.
- No attachments on first email.
- No HTML formatting — plain text looks personal.
- Subject line: 4-7 words, lowercase, looks like a real email ("quick question about [their company]")
- Follow up 3 times max, spaced 3-5 days apart. Each follow-up adds new information, not "just checking in."

### Newsletters

Use when: Building ongoing relationships with an audience.

**Format options:**

- **Curated**: 5-7 links with 1-2 sentence commentary each
- **Essay**: One deep topic, 500-800 words, with a personal angle
- **Hybrid**: Short personal note + curated links + one featured piece
- **Product update**: What shipped, what's next, one user story

**Newsletter rules:**

- Consistent send time (readers build habits)
- Subject line previews the value, not clickbait ("This week: 3 tools that cut our deploy time in half")
- Preheader text (the preview text after the subject line) — write it intentionally, don't let it default to "View in browser"
- Always provide an easy way to reply — engagement signals help deliverability

## Subject Line Craft

The subject line determines whether everything else matters. Follow these principles:

- **Specificity wins**: "How we cut churn by 34%" beats "Reducing customer churn"
- **Numbers work**: "5 mistakes" outperforms "common mistakes"
- **Questions create curiosity**: "Are you making this onboarding mistake?"
- **Personalization helps**: Including the recipient's name or company lifts open rates 20%+
- **Length**: 6-10 words. Under 50 characters for mobile.
- **A/B test**: Always write 2 subject lines. Test on 20% of the list, send the winner to the rest.

## Segmentation Strategy

When the user has a list, help them segment it:

- **By behavior**: Opened last 3 emails vs. hasn't opened in 30 days
- **By stage**: New subscriber vs. free user vs. paying customer
- **By interest**: What content/pages they've engaged with
- **By value**: High-value customers get different messaging than free users

Use `memory_store` to save segment definitions and campaign state for multi-session work.

## Technical Notes

- Use `template_render` for emails with dynamic content (name, company, custom fields)
- Use `file_write` to output email sequences as structured markdown files
- Research competitors' email strategies with `web_search` before designing campaigns
- Include plain text versions alongside HTML — some clients strip HTML, and plain text often converts better for B2B

## Output Format

For each email in a sequence, deliver:

- Subject line (with A/B variant)
- Preheader text
- Email body (formatted as the recipient would see it)
- CTA button text and destination
- Send timing (day and time relative to trigger event)
- Segment targeting (who receives this email)
