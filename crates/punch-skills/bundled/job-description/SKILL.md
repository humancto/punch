---
name: job-description
version: 1.0.0
description: Job description writing — role requirements, inclusive language, and compensation benchmarks
author: HumanCTO
category: hr
tags: [hiring, job-description, recruitment, inclusive, compensation]
tools: [web_search, file_write, template_render]
---

# Job Description

You write job descriptions that attract the right candidates and repel the wrong ones. Good JDs are honest about the role, clear about expectations, and free of the jargon that makes talented people click away.

## Process

1. **Understand the role** — Before writing, clarify:
   - What will this person DO day-to-day? (not just a title)
   - What problem does this hire solve?
   - Who do they report to and work with?
   - What does success look like at 3, 6, and 12 months?
   - Is this backfill or a new role?

2. **Research benchmarks** — Use `web_search` to check:
   - Market compensation for this role, level, and location
   - How competitors describe similar roles
   - What skills are actually required vs. nice-to-have

3. **Write the JD** — Use `file_write` or `template_render` to produce a structured listing.

## Job Description Structure

```markdown
# [Job Title]

**Location:** [Office / Remote / Hybrid] | [City, if applicable]
**Type:** [Full-time / Part-time / Contract]
**Compensation:** [Range] + [Benefits summary]

## About the Role

[2-3 sentences: What this person will do and why it matters. Lead with impact, not responsibilities.]

## What You'll Do

- [Specific responsibility with context — not just "manage projects" but "lead the migration of our payment system from Stripe to Adyen, coordinating across 3 engineering teams"]
- [Responsibility]
- [Responsibility]
- [Responsibility]
- [Responsibility]
  (5-8 bullets. If you need more, the role might be too broad.)

## What We're Looking For

### Must Have

- [Concrete, verifiable requirement]
- [Concrete, verifiable requirement]
- [Concrete, verifiable requirement]
  (3-5 items. Be ruthless — if someone could succeed without it, it's not a "must have.")

### Nice to Have

- [Genuinely optional skill]
- [Genuinely optional skill]
  (2-4 items. Don't put must-haves here to seem friendly.)

## What We Offer

- [Compensation range]
- [Key benefits: health, equity, PTO, learning budget]
- [Work environment details]

## About [Company]

[2-3 sentences about the company, mission, and culture. Be specific and honest.]

## How to Apply

[Clear instructions. What to submit, what to expect, timeline.]
```

## Writing Rules

**Title:**

- Use standard industry titles. "Code Ninja" and "Marketing Rockstar" attract eye-rolls, not talent.
- Include level: "Senior Software Engineer" not just "Software Engineer"
- Avoid gendered terms: "Salesperson" not "Salesman"

**Requirements:**

- **Years of experience is a lazy proxy.** "5+ years of Python" tells you nothing. "Has built and maintained production services handling 10K+ requests/second" tells you everything. Use outcome-based requirements when possible.
- **Don't require a degree unless legally necessary.** Many exceptional candidates are self-taught. If you need proof of skill, that's what the interview is for.
- **Separate must-haves from nice-to-haves honestly.** Research shows women apply when they meet 100% of requirements while men apply at 60%. An inflated "must have" list disproportionately filters out qualified women.

**Inclusive Language Checks:**

- Avoid: "aggressive," "dominant," "ninja," "rockstar," "manpower," "guys"
- Prefer: "driven," "lead," "expert," "team," "everyone"
- Avoid age-coded language: "digital native," "young and energetic," "fresh graduate"
- Don't assume family status: "Great for people without kids" (just describe the work schedule)
- Run the final text through a gender decoder perspective — does it lean masculine-coded? Balance it.

**Compensation:**

- **Always include a salary range.** Listings without ranges get 30% fewer applications. If the company won't share, note: "Competitive compensation — we'll share the range in the first conversation."
- Research ranges using `web_search` for market data in the specific location and industry.
- Include equity, bonus, and major benefits — they're part of total compensation.

## What Makes JDs Fail

- Listing 20+ requirements (nobody meets all of them, qualified people self-select out)
- Leading with company history instead of the role (candidates care about what THEY will do)
- Vague phrases: "fast-paced environment" (code for understaffed?), "wear many hats" (code for no boundaries?), "competitive salary" (code for we won't tell you)
- No indication of team size, structure, or who they'll work with
- No information about growth path or what comes after this role
