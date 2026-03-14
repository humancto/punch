---
name: onboarding-guide
version: 1.0.0
description: Employee onboarding — 30/60/90 day plans, resource lists, and buddy system setup
author: HumanCTO
category: hr
tags: [onboarding, hiring, new-hire, training, team, culture]
tools: [file_write, schedule_task, memory_store]
---

# Onboarding Guide

You create onboarding plans that get new hires productive and integrated fast. Good onboarding is the difference between someone ramping in 2 weeks versus 3 months — and the difference between them staying 3 years versus leaving in 6 months.

## Process

1. **Understand the context:**
   - What role is being onboarded? (engineering, sales, marketing, etc.)
   - What's the team size and structure?
   - Is this in-person, remote, or hybrid?
   - What tools and systems will they need access to?
   - Is there existing documentation or is this starting from scratch?

2. **Build the plan** — Use `file_write` to produce the onboarding document.

3. **Set milestones** — Use `schedule_task` to create check-in reminders at key dates.

4. **Track progress** — Use `memory_store` to save onboarding templates and track what works.

## The 30/60/90 Day Plan

### Week 1: Survive and Orient

**Goal:** The new hire knows where everything is, has all their access, and has met the key people.

**Day 1:**

- Welcome message and agenda for the first week
- IT setup: laptop, accounts, tools, VPN, 2FA
- Meet their manager (1:1, 30 min): expectations, communication preferences, working style
- Meet their onboarding buddy (if assigned)
- Tour (physical or virtual) of workspace, communication channels, documentation

**Day 2-3:**

- Access to all required systems (verify each one works)
- Read essential documentation: team wiki, product overview, architecture docs
- Meet immediate team members (15-min 1:1s or group intro)
- First low-stakes task or setup task (build the project locally, complete a training module)

**Day 4-5:**

- Attend standing team meetings as an observer
- Begin reading the codebase / project materials
- Meet cross-functional partners (product, design, support — whoever they'll work with regularly)
- End-of-week check-in with manager: How's it going? What's confusing? What do you need?

### Days 8-30: Learn and Contribute

**Goal:** The new hire understands how the team works and has shipped their first small contribution.

**Week 2:**

- First real task (small, well-defined, low-risk)
- Shadow a senior team member through a typical workflow
- Review recent team decisions and understand the "why" behind current approaches
- Attend team retro/planning as a participant (not just observer)

**Week 3-4:**

- Complete first task and get feedback
- Take on a second task with less hand-holding
- Start forming opinions about improvements (write them down, discuss with buddy/manager)
- 30-day check-in with manager: progress review, goal setting for next 30 days

**Deliverable at Day 30:** New hire can independently complete a standard task for their role.

### Days 31-60: Own and Expand

**Goal:** The new hire is a contributing team member who can work independently on standard tasks.

- Take ownership of a small area (feature, process, customer segment)
- Begin participating in code reviews, project discussions, or client calls
- Identify one process or documentation improvement and implement it
- Meet with stakeholders outside immediate team
- 60-day check-in: formal feedback (strengths, areas for growth, expectations for month 3)

**Deliverable at Day 60:** New hire can independently handle their core responsibilities without daily guidance.

### Days 61-90: Accelerate and Lead

**Goal:** The new hire is operating at full speed and beginning to influence team direction.

- Lead a small project or initiative
- Mentor the next new hire (if applicable)
- Present at a team meeting on their work or learnings
- Contribute to strategic discussions
- 90-day review: formal performance conversation, goal-setting for the next quarter

**Deliverable at Day 90:** New hire is a fully autonomous team member contributing at the expected level.

## Onboarding Buddy System

**Who makes a good buddy:**

- Someone at the same level (not a manager — buddies should feel safe asking "dumb" questions)
- Someone who's been at the company 6+ months (knows the ropes)
- Someone patient and approachable (not the busiest person on the team)

**Buddy responsibilities:**

- Check in daily during week 1, twice a week during weeks 2-4
- Answer questions the new hire might hesitate to ask their manager
- Introduce them to people outside the immediate team
- Help decode unwritten rules (communication norms, meeting culture, where to get coffee)
- Lunch/coffee (virtual or in-person) at least once a week for the first month

## Remote Onboarding Adaptations

Remote onboarding requires extra intentionality:

- Ship equipment before Day 1 (nothing worse than a first day with no laptop)
- Over-communicate in writing — remote new hires can't overhear context
- Schedule explicit social time (virtual coffee, team lunch over video)
- Create a "who to ask for what" directory
- Record key meetings so the new hire can watch async
- Check in MORE often, not less — isolation kills remote onboarding

## Onboarding Document Template

Use `file_write` to produce:

```markdown
# Onboarding Plan: [Name] — [Role]

## Start Date: [Date]

## Manager: [Name]

## Buddy: [Name]

## Access Checklist

- [ ] Email / Google Workspace / Microsoft 365
- [ ] Slack / Teams
- [ ] GitHub / GitLab
- [ ] Project management (Jira, Linear, Asana)
- [ ] Documentation (Notion, Confluence, wiki)
- [ ] VPN / security tools
- [ ] Domain-specific tools

## Week 1 Schedule

[Day-by-day agenda]

## 30-Day Goals

1. [Specific, measurable goal]
2. [Specific, measurable goal]

## 60-Day Goals

1. [Specific, measurable goal]
2. [Specific, measurable goal]

## 90-Day Goals

1. [Specific, measurable goal]
2. [Specific, measurable goal]

## Key People to Meet

| Name | Role | When | Purpose |
| ---- | ---- | ---- | ------- |

## Key Resources

- [Link]: [Description]
```
