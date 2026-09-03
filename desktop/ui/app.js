// OrgBot desktop UI: manage a bot library and rooms (group chats of those
// bots), start a room on the embedded host, and watch its board and chat live.

const $ = (id) => document.getElementById(id);

async function invoke(cmd, args) {
  if (!window.__TAURI__) throw new Error('not running inside the OrgBot desktop shell');
  return window.__TAURI__.core.invoke(cmd, args || {});
}

const PROVIDERS = [['codex','Codex CLI'],['claude','Claude CLI'],['grok','Grok CLI'],['api','Anthropic API'],['scripted','Scripted demo']]
  .map(([v, l]) => ({ v, l }));
const ROLES = [['writer','Writer'],['coordinator','Coordinator'],['reviewer','Inspector']].map(([v, l]) => ({ v, l }));

// state
let bots = [];
let rooms = [];

// tabs
document.querySelectorAll('#tabs button').forEach((btn) => {
  btn.onclick = () => {
    document.querySelectorAll('#tabs button').forEach((b) => b.classList.remove('active'));
    btn.classList.add('active');
    const panel = btn.dataset.panel;
    document.querySelectorAll('.panel').forEach((p) => p.classList.remove('active'));
    $('panel-' + panel).classList.add('active');
    if (panel === 'bots') renderBots();
    if (panel === 'rooms') renderRooms();
  };
});

function banner(text, isError) {
  const chip = $('hostChip');
  chip.textContent = text;
  chip.style.background = isError ? '#5a2424' : '#263a2c';
}

// ---- bots
function providerLabel(v) {
  const hit = PROVIDERS.find((p) => p.v === v);
  return hit ? hit.l : v;
}

async function refreshBots() { bots = await invoke('bots_list'); renderBots(); }

function renderBots() {
  const wrap = $('botList');
  if (!bots.length) { wrap.innerHTML = '<div class="card muted">No bots yet — create your first one.</div>'; return; }
  wrap.innerHTML = bots.map((b) =>
    '<div class="card"><div class="row"><strong>@' + esc(b.handle) + '</strong>' +
    '<span class="chip">' + providerLabel(b.provider) + '</span>' +
    (b.model ? '<span class="muted">' + esc(b.model) + '</span>' : '') +
    '<span class="spacer"></span>' +
    '<button class="ghost" data-edit="' + b.id + '">Edit</button>' +
    '<button class="ghost" data-del="' + b.id + '">Delete</button></div>' +
    '<div class="muted" style="margin-top:6px">' + esc(b.persona || 'no persona') + '</div></div>').join('');
  wrap.querySelectorAll('[data-edit]').forEach((b) => (b.onclick = () => botForm(b.dataset.edit)));
  wrap.querySelectorAll('[data-del]').forEach((b) => (b.onclick = () => deleteBot(b.dataset.del)));
}

document.getElementById('newBotBtn').onclick = () => botForm(null);

function botForm(id) {
  const existing = bots.find((b) => b.id === id);
  const wrap = $('botFormWrap');
  wrap.innerHTML = '<div class="card"><h2>' + (existing ? 'Edit bot' : 'New bot') + '</h2>' +
    '<label>Name</label><input id="bfName" value="' + esc(existing ? existing.name : '') + '" />' +
    '<label>Handle (@mention)</label><input id="bfHandle" value="' + esc(existing ? existing.handle : '') + '" />' +
    '<label>System prompt / persona</label><textarea id="bfPersona" rows="4">' + esc(existing ? existing.persona : '') + '</textarea>' +
    '<div class="grid2"><div><label>Provider</label><select id="bfProvider">' +
    PROVIDERS.map((p) => '<option value="' + p.v + '"' + (existing && existing.provider === p.v ? ' selected' : '') + '>' + p.l + '</option>').join('') +
    '</select></div><div><label>Model (empty = default)</label><input id="bfModel" value="' + esc(existing ? existing.model : '') + '" placeholder="claude-sonnet-5 / deepseek-chat…" /></div></div>' +
    '<div class="row" style="margin-top:12px"><button class="primary" id="bfSave">Save bot</button>' +
    '<button class="ghost" id="bfCancel">Cancel</button></div><div id="bfErr" class="err"></div></div>';
  $('bfCancel').onclick = () => (wrap.innerHTML = '');
  $('bfSave').onclick = async () => {
    try {
      await invoke('bot_save', { id: id || null, name: $('bfName').value, handle: $('bfHandle').value,
        persona: $('bfPersona').value, provider: $('bfProvider').value, model: $('bfModel').value });
      wrap.innerHTML = '';
      await refreshBots();
    } catch (err) { $('bfErr').textContent = err && (err.message || String(err)); }
  };
}

async function deleteBot(id) { await invoke('bot_delete', { id }); await refreshBots(); }

// ---- rooms
async function refreshRooms() { rooms = await invoke('rooms_list'); renderRooms(); }

function renderRooms() {
  const wrap = $('roomList');
  if (!rooms.length) { wrap.innerHTML = '<div class="card muted">No rooms yet — open a group chat of your bots.</div>'; return; }
  wrap.innerHTML = rooms.map((r) =>
    '<div class="card"><div class="row"><strong>' + esc(r.name) + '</strong>' +
    '<span class="chip">' + r.bot_count + ' bots</span>' +
    '<span class="chip">' + r.task_count + ' tasks</span>' +
    '<span class="spacer"></span>' +
    '<button class="primary" data-start="' + r.id + '">Start</button>' +
    '<button class="ghost" data-del="' + r.id + '">Delete</button></div>' +
    '<div class="muted" style="margin-top:6px">Goal: ' + esc(r.goal) + '</div>' +
    (r.path ? '<div class="muted">Path: ' + esc(r.path) + '</div>' : '') + '</div>').join('');
  wrap.querySelectorAll('[data-start]').forEach((b) => (b.onclick = () => startRoom(b.dataset.start)));
  wrap.querySelectorAll('[data-del]').forEach((b) => (b.onclick = () => deleteRoom(b.dataset.del)));
}

document.getElementById('newRoomBtn').onclick = () => roomForm();

function roomForm() {
  const wrap = $('roomFormWrap');
  const botRows = bots.map((b, i) =>
    '<div class="botrow"><input type="checkbox" id="rb_' + i + '" checked />' +
    '<span class="who">@' + esc(b.handle) + '</span><select id="rr_' + i + '">' +
    ROLES.map((r) => '<option value="' + r.v + '">' + r.l + '</option>').join('') +
    '</select></div>').join('');
  wrap.innerHTML = '<div class="card"><h2>New room — a group chat that works a folder</h2>' +
    '<div class="grid2"><div><label>Room name</label><input id="rfName" placeholder="release notes" /></div>' +
    '<div><label>Project path (the bots work here)</label><input id="rfPath" placeholder="/home/me/my-project" /></div></div>' +
    '<label>Goal / task</label><textarea id="rfGoal" rows="3" placeholder="Write clear release notes for v1.3 covering every change…"></textarea>' +
    '<label>Who is in the room</label>' + (botRows || '<div class="muted">Create some bots first.</div>') +
    '<div class="row" style="margin-top:12px"><button class="primary" id="rfSave">Save room</button>' +
    '<button class="primary" id="rfStart">Save & Start</button>' +
    '<button class="ghost" id="rfCancel">Cancel</button></div><div id="rfErr" class="err"></div></div>';
  $('rfCancel').onclick = () => (wrap.innerHTML = '');
  const collect = () => {
    const slots = [];
    bots.forEach((b, i) => {
      const box = $('rb_' + i);
      if (box && box.checked) slots.push({ bot_id: b.id, roles: [$('rr_' + i).value] });
    });
    return { name: $('rfName').value, path: $('rfPath').value, goal: $('rfGoal').value,
      tasks: [$('rfGoal').value].filter((t) => t.trim()), bots: slots };
  };
  const run = async (start) => {
    try {
      const data = collect();
      const saved = await invoke('room_save', { id: null, name: data.name, goal: data.goal, path: data.path,
        tasks: data.tasks, bots: data.bots });
      wrap.innerHTML = '';
      await refreshRooms();
      if (start) await startRoom(saved.id);
    } catch (err) { $('rfErr').textContent = err && (err.message || String(err)); }
  };
  $('rfSave').onclick = () => run(false);
  $('rfStart').onclick = () => run(true);
}

async function deleteRoom(id) { await invoke('room_delete', { id }); await refreshRooms(); }

// ---- live room
let ws = null;
let currentRoomName = '';

function enterLive(name, goal, url) {
  currentRoomName = name;
  $('liveName').textContent = name;
  $('liveGoal').textContent = goal || '';
  $('main').classList.add('hasLive');
  $('live').classList.add('show');
  connect(url);
}

function exitLive() {
  if (ws) { ws.close(); ws = null; }
  $('live').classList.remove('show');
  $('main').classList.remove('hasLive');
  banner('no room running');
}

async function startRoom(id) {
  banner('starting room…');
  try {
    const info = await invoke('room_start', { id });
    banner('room running');
    const room = rooms.find((r) => r.id === id) || {};
    enterLive(room.name || 'Room', room.goal, info.ws_url);
  } catch (err) { banner('Start failed: ' + ((err && (err.message || err)) || 'unknown error'), true); }
}

document.getElementById('stopRoomBtn').onclick = async () => { await invoke('host_stop', {}); exitLive(); };

// ws store
const store = { room: null, agents: new Map(), tasks: new Map(), messages: [], phase: null };

function handleMessage(raw) {
  let msg;
  try { msg = JSON.parse(raw); } catch { return; }
  if (msg.type === 'snapshot') applySnapshot(msg);
  else if (msg.type === 'event') applyEnvelope(msg.envelope);
  else if (msg.type === 'phase') setPhase(msg.phase);
  else if (msg.type === 'closed') setPhase('completed');
  else if (msg.type === 'error') resync();
}

function resync() { if (ws && ws.readyState === 1) ws.send(JSON.stringify({ type: 'resync' })); }

function applySnapshot(snap) {
  const room = snap.room || {};
  store.room = room;
  store.agents.clear();
  (snap.roster || []).forEach((a) => store.agents.set(a.id, a));
  store.tasks.clear();
  (snap.tasks || []).forEach((t) => store.tasks.set(t.id, t));
  store.messages = [];
  store.phase = room.phase;
  renderAll();
}

function applyEnvelope(env) {
  const e = (env && env.event) || {};
  if (e.event === 'message_posted') { store.messages.push(e.message || {}); renderChat(); }
  else if (e.event === 'task_created' && e.task) { store.tasks.set(e.task.id, e.task); renderBoard(); }
  else if (e.event === 'task_transitioned' && e.task) {
    const t = store.tasks.get(e.task);
    if (t) t.state = e.to;
    renderBoard();
  } else if (e.event === 'agent_status_changed' && e.agent) {
    const a = store.agents.get(e.agent);
    if (a) a.status = e.to;
  }
}

function setPhase(phase) { store.phase = phase; renderHeader(); }

function esc(s) {
  const map = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(s).replace(/[&<>"']/g, (c) => map[c] || c);
}

function agentHandle(id) {
  const a = store.agents.get(id);
  return a ? a.handle : (id || '').slice(0, 10);
}

function renderAll() { renderHeader(); renderChat(); renderBoard(); }

function renderHeader() {
  const spec = (store.room && store.room.spec) || {};
  $('liveName').textContent = spec.name || currentRoomName;
  $('liveGoal').textContent = spec.goal || '';
  $('livePhase').textContent = store.phase || '';
  $('pauseBtn').textContent = store.phase === 'paused' ? 'Resume' : 'Pause';
}

function renderChat() {
  const box = $('lmessages');
  if (!store.messages.length) { box.innerHTML = '<div class="muted">No messages yet</div>'; return; }
  box.innerHTML = store.messages.map((m) => {
    const a = m.author || {};
    const who = a.kind === 'human' ? 'you' : '@' + agentHandle(a.agent || a.id);
    return '<div class="bmsg"><div class="who">' + esc(who) + '</div><div class="body">' + esc(m.body || '') + '</div></div>';
  }).join('');
  box.scrollTop = box.scrollHeight;
}

const COLOR = { claimed: '#6d8bff', started: '#6d8bff', in_progress: '#6d8bff',
  ready: '#7c8591', in_review: '#d0a64e', approved: '#93d9a0', submitted: '#93d9a0',
  delivered: '#93d9a0', done: '#93d9a0', blocked: '#e08a7a', failed: '#e08a7a' };

function renderBoard() {
  const list = [...store.tasks.values()].sort((a, b) => a.title.localeCompare(b.title));
  const wrap = $('lboard');
  if (!list.length) { wrap.innerHTML = '<div class="muted">Empty board</div>'; return; }
  wrap.innerHTML = list.map((t) =>
    '<div class="tcard" style="border-left-color:' + (COLOR[t.state] || '#7c8591') + '">' +
    '<strong>' + esc(t.title) + '</strong><div class="muted">' + t.state +
    (t.assignee ? ' · ' + esc(agentHandle(t.assignee)) : '') + '</div></div>').join('');
}

function connect(url) {
  if (ws && ws.readyState <= 1) return;
  ws = new WebSocket(url);
  ws.onopen = () => { $('connChip').textContent = 'connected'; };
  ws.onclose = () => { $('connChip').textContent = 'disconnected'; };
  ws.onmessage = (ev) => handleMessage(ev.data);
}

function send(obj) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj)); }

document.getElementById('sendBtn').onclick = sendChat;
document.getElementById('msgBox').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendChat(); }
});

function sendChat() {
  const body = $('msgBox').value.trim();
  if (!body) return;
  send({ type: 'send_message', author: 'You', body });
  $('msgBox').value = '';
}

document.getElementById('pauseBtn').onclick = () => {
  send({ type: store.phase === 'paused' ? 'resume' : 'pause' });
};

// boot
(async () => {
  try {
    await Promise.all([refreshBots(), refreshRooms()]);
  } catch (err) {
    banner('desktop shell needed: ' + ((err && (err.message || err)) || err), true);
  }
})();
