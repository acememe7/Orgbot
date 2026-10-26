# OrgBots website privacy assessment

**Checked:** 26 October 2026

**Scope:** Public static website only. This is a factual and legal-issue assessment, not a compliance certification or legal opinion.

## Current facts and verification

The owner identifies The AECE Studio as a business name registered in Nigeria and the OrgBots website operator. The site has no accounts, forms, payments, or user uploads. Its contact address is `theaece@theaecestudio.com`.

Cloudflare proxies the site; Hermes VPS hosts the Nginx origin. The site privacy page states that Cloudflare and the host process IP address, request path, browser information, and request time for delivery, security, and troubleshooting. Origin logs rotate daily with 14 retained rotations. This describes the configured rotation count, not a verified exact deletion interval or Cloudflare log-retention period. The current notice identifies Cloudflare security processing and possible processing outside Nigeria.

Optional Cloudflare Web Analytics is **disabled**. The site does not load the beacon or store an analytics consent preference. The disabled path has been tested for no storage and no external network request. A future opt-in gate has been prepared; tests for cross-tab withdrawal and preference expiry pass. A Web Analytics property exists, but no visitor measurements are sent while the feature remains disabled. The live `Cache-Control: public, no-transform` header prevents automatic response injection, and the site's CSP denies the external beacon. The token is intentionally not recorded here. Astra's review explicitly leaves any beacon enablement gated on qualified interpretation of GAID Article 31; this assessment does not clear that gate.

The current privacy page names the operator and contact, describes website data and purposes, states a legitimate-interest basis for necessary delivery/security processing, explains possible foreign processing, says analytics is disabled, lists rights, and links to the NDPC. It says support correspondence is kept while a request is being resolved or where needed for a legal claim/obligation; a fixed mailbox retention period and final mailbox provider remain unknown. Public MX records point to Cloudflare mail-routing hosts, which do not identify the final mailbox or its storage provider.

Website recovery copies are stored in the owner's private Google Drive folder and exclude visitor logs, contact messages, application rooms, and credentials. The remote archive was downloaded, hash-matched, and restored in isolation as documented in `docs/orgbots-recovery.md`. Google Drive is the owner's explicit destination selection; this is not Google Cloud Storage. Search Console ownership is verified and the sitemap reports five URLs successfully. Latest operator checks report direct-origin requests return 403 and the proxied ACME challenge returns 200. These results are operational evidence, not a privacy or legal certification.

## Legal assessment

**Nigeria is the primary jurisdiction.** The owner confirms Nigerian business-name registration and identifies the business as the site operator. That makes the Nigeria Data Protection Act directly relevant; confirm the precise domicile/residence/operation and processing facts against its territorial test rather than treating registration alone as a complete legal analysis. The public site's collection of IP/request metadata and any contact messages is personal-data processing.

**Necessary connection/security logging.** A preliminary legitimate-interest balance supports the stated, narrow purpose: delivering requested static pages, preventing abuse, and diagnosing failures. Connection/request metadata is reasonably necessary to operate and protect the host; the site has no account, form, advertising, or user-upload flow; origin logs are limited to 14 daily rotations; and the privacy notice explains the purpose and data categories. This supports plausibility of legitimate interests for minimum origin logs, subject to the operator's operational approval and an accurate record of the balance. It does not justify unrelated analytics or broader retention. Cloudflare's separate security records, fields, access, and retention still need confirmation. This is an assessment of the stated facts, not a claim of legal approval or a prescribed statutory LIA form.

**DPIA.** NDPA section 28 and GAID Article 28 require an assessment where processing is likely to create high risk to rights and freedoms. The current low-complexity static site, without accounts, forms, payments, uploads, or analytics, does not on the documented facts alone show a likely high-risk processing activity. GAID Article 31 separately requires a prior DPIA for a controller/processor that deploys data-processing software to track a data subject or enable a communication link while processing personal data. Whether a future Cloudflare measurement beacon meets that test remains unresolved. Keep analytics disabled until qualified review determines whether Article 31 applies and, if so, a DPIA is completed before deployment. No broad claim that every static website requires a DPIA is supported here, and no Article 31 clearance is claimed.

**Optional analytics.** If reconsidered, the prepared opt-in design is a conservative choice, not a legal determination. Reconfirm before enabling: what the beacon transmits (including normal request metadata), Cloudflare's role and retention, transfer mechanism, the consent record and expiry, no request before affirmative allowance, refusal behavior, and withdrawal across tabs/reloads. Keep the notice and policy accurate and ensure the studio-wide “no tracking on our websites” statement is revised or scoped before any site sends analytics.

**Transfers and retention remain open.** Cloudflare and Hermes VPS are named providers, but their exact processing locations, contractual roles, subprocessors, and applicable Nigeria Data Protection Act Part VIII transfer mechanism have not been documented here. Confirm these facts and the final email destination before claiming transfer compliance. Confirm actual retention for Cloudflare security records, mailbox correspondence and backups, and report a concrete period or a clear event-based schedule. Do not present 14 origin-log rotations as the retention of Cloudflare or email records.

## Open items

- Confirm the proprietor/underlying legal person behind the registered business name if needed for full controller identification; do not invent a company suffix, registration number, address, certification, or contact detail.
- Identify the final mailbox provider, access controls, data location, mail/backup retention, and the person responsible for privacy requests.
- Record Cloudflare/Hermes VPS processing locations, contractual roles/subprocessors, transfer safeguards, Cloudflare security-log fields/retention, and the exact active origin-log retention interval.
- Keep the Article 31 determination as a gate before enabling the analytics beacon. Reassess EU/UK or other local rules only if the owner targets those markets.

## Primary sources

- [Nigeria Data Protection Act 2023](https://ndpc.gov.ng/wp-content/uploads/2024/03/Nigeria_Data_Protection_Act_2023.pdf) and the NDPC's [territorial-scope FAQ](https://ndpc.gov.ng/faqs/).
- [Nigeria Data Protection Act-GAID 2025](https://ndpc.gov.ng/wp-content/uploads/2025/07/NDP-ACT-GAID-2025-MARCH-20TH.pdf): Article 19 (tracking tools), Article 27 (privacy information, including identity, lawful basis, purposes, recipients, rights and complaint route), Article 28 (risk-based DPIA), and Article 31 (software deployment for tracking/communication while processing personal data).
- The Act's section 46 complaint right, the [NDPC rights overview](https://ndpc.gov.ng/), and its [data-protection/privacy-violation reporting portal](https://services.ndpc.gov.ng/breach/).
