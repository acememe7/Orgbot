# OrgBots 2.1.0 live room run

Date: 28 to 29 October 2026, 20:10 to 04:11 WAT (UTC+1).
Data: [2026-10-29-live-room-run.json](2026-10-29-live-room-run.json).

## What ran

One real project room, the owner's browser game, with 10 bots on 5 providers
working its task board overnight. The room ran on development builds until
03:07 and on the 2.1.0 package afterwards. Counters are OrgBots' per-turn usage
records as each CLI or API reported them. They are not invoices or quota
readings.

| Bot | Provider and model | Roles | Turns | Uncached input and cache write | Cache read | Output |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| Julian | Claude Code · Opus 5.5 | architect, coordinator | 23 | 452,268 | 17,370,294 | 66,394 |
| Sol | Codex CLI · GPT-6.1 Sol | engineer, ops | 17 | 1,068,967 | 46,626,432 | 202,837 |
| Devon | DeepSeek API · DeepSeek Flash | engineer, backend | 16 | 1,222,836 | 13,694,464 | 153,215 |
| Mira | Codex CLI · GPT-6 Luna | engineer, frontend | 15 | 1,365,765 | 65,696,768 | 391,702 |
| Finn | Grok CLI · Grok 4.7 | engineer | 14 | 1,010,578 | 17,361,152 | 211,694 |
| Quinn | Ollama Cloud · GLM 5.3 Flash | QA | 13 | 1,106,502 | 7,871,680 | 302,440 |
| Astra | Codex CLI · GPT-6 Astra | security, reviewer, coordinator | 8 | 402,490 | 15,202,048 | 36,696 |
| Miles | Grok CLI · Grok 4.7 | support | 6 | 518,720 | 2,515,328 | 22,396 |
| Ellis | Claude Code · Haiku 5.5 | writer, reviewer | 6 | 445,089 | 4,568,898 | 31,036 |
| Rhea | Ollama Cloud · DeepSeek v4.1 Flash | researcher, analyst | 0 | 0 | 0 | 0 |
| **Total** | 5 providers | | **118** | **7,593,215** | **190,907,064** | **1,418,410** |

Cache reads are long turns rereading their own context, which providers bill
or count at a reduced rate. Rhea had no turns: nothing in the run called for
her researcher and analyst roles.

## Work delivered

15 tasks were created. These 8 were delivered after an independent review by another bot:

- Audit the game, its repository and its release path
- Reconcile the next work plan with the owner's updated progress notes
- QA baseline and acceptance matrix for the game's modes
- Reconcile protocol and hosting documentation before a release gate
- Correct a line in the studio website's FAQ on the owner's server (owner decision)
- Remove an unused GitHub Pages workflow and tidy the docs (owner decision)
- Queue whole parties of up to four players in public matches
- Assess and contain a server release that went live during a review hold

The bots and the owner exchanged 101 bot messages and 22 owner messages. The remaining tasks were still in progress or in review when the record was taken.

## The hour on the 2.1.0 package

From 03:07 to 04:11 the room ran on the final 2.1.0 package. Eight bots took 34 turns and wrote 357,039 output tokens. No claim was released by mistake. One turn failed: a provider-side Codex capacity error; the bot recovered on retry after 11 minutes. One more task was delivered after review.

## Account usage the owner observed

| Provider | Figure | Source |
| --- | --- | --- |
| Codex (ChatGPT plan, shared weekly limit) | 97% of the weekly limit left at 02:13 | owner screenshot; owner estimate for the run: about 1% of the weekly limit |
| Ollama Cloud (Pro plan) | $0.37 of monthly credits used, 144 GLM 5.3 Flash requests this month, at 02:12 | owner screenshot |
| DeepSeek API | $4.92 over the last 30 days on the key used, all activity on that key | owner screenshot; owner estimate for the run: about $0.25 |
| Claude (weekly limit) | 6% of the weekly limit, which the owner attributes mostly (about 5 points) to a separate Claude Code development session, not the room | owner report, no screenshot |
| Grok (premium plan, 5-hour window) | 7% of the 5-hour window | owner report, no screenshot |

## Problems found during the run

11 turns failed: 6 on sign-ins (an expired Claude sign-in three times, and three Codex sign-in failures after a development test overwrote the owner's Codex sign-in file, since fixed and covered by a test), 3 CLI exits on prompts that began with a dash (one Claude, two Grok; fixed), 1 turn time limit (removed), 1 provider-side capacity error on Codex, treated as temporary and retried.
Before 2.1.0, claims on work were released by mistake 18 times: during long turns, while a bot waited on a teammate, and while it waited for a free slot. 2.1.0 fixes all three, and each fix has a test that fails without it.

## Limits

- One room and one night; not a controlled comparison with a single agent.
- Dashboards are account-wide and include other activity on the same accounts.
- Token counters are as reported by each provider and do not establish a dollar charge or quota share.
- The room ran on development builds before 03:07 and on 2.1.0 afterwards.
- Handoffs carry a deadline the sending bot sets. In the final hour two of four handoffs expired before the recipient acted, one while the recipient was in a long turn on other work.
