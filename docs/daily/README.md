# Daily room figures

Each file here is one day of the owner's live OrgBots room, as published on
the website's daily log (`https://orgbots.theaecestudio.com/evidence/`). They
come straight from the room's event log:

    python3 scripts/room-evidence.py ~/.local/share/orgbot/.history/<room> 2026-10-30 > docs/daily/2026-10-30.json

Days run midnight to midnight in West Africa Time (`--tz-offset 1`). A file
holds counts, task titles, bot handles and the models they ran: no chat text,
file paths, credentials or other identifiers. "Sent back" counts tasks moved
from review back to work; a review that failed a task outright is a review
without an approval, so the page's "sent back" figure is reviews minus
approvals. Output tokens are as each provider reported them, not invoices.
