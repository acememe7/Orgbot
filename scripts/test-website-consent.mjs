import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const source = await readFile(new URL('../website/site.js', import.meta.url), 'utf8');
const CONSENT_KEY = 'orgbots-analytics-consent-v1';
const SESSION_KEY = 'orgbots-analytics-disabled-v1';
const LIFETIME = 180 * 24 * 60 * 60 * 1000;
const BEACON_SRC = 'https://static.cloudflareinsights.com/beacon.min.js';
const BEACON_TOKEN = '46be099b90994f12997c7c0b9019c0f0';

class NodeFixture {
  constructor() {
    this.hidden = true;
    this.textContent = '';
    this.dataset = {};
    this.attributes = new Map();
    this.listeners = new Map();
    this.focusCount = 0;
  }

  addEventListener(type, callback) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(callback);
    this.listeners.set(type, listeners);
  }

  fire(type = 'click') {
    this.ownerDocument.activeElement = this;
    for (const callback of this.listeners.get(type) ?? []) {
      callback({ target: this, currentTarget: this });
    }
  }

  focus() {
    this.focusCount += 1;
    this.ownerDocument.activeElement = this;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }
}

function storage(initial = {}, { failGet = false, failSet = false, failRemove = false } = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    reads: 0,
    writes: 0,
    getItem(key) {
      this.reads += 1;
      if (failGet) throw new Error('storage read denied');
      return values.has(key) ? values.get(key) : null;
    },
    setItem(key, value) {
      this.writes += 1;
      if (failSet) throw new Error('storage write denied');
      values.set(key, String(value));
    },
    removeItem(key) {
      if (failRemove) throw new Error('storage remove denied');
      values.delete(key);
    },
  };
}

function preference(choice, savedAt = Date.now()) {
  return JSON.stringify({
    version: 1,
    choice,
    savedAt,
    expiresAt: savedAt + LIFETIME,
  });
}

function boot({ local, session = storage(), hash = '', enabled = false } = {}) {
  const document = {
    activeElement: new NodeFixture(),
    documentElement: { classList: { add() {} } },
    visibilityState: 'visible',
    listeners: new Map(),
    head: {
      scripts: [],
      appendChild(script) { this.scripts.push(script); },
    },
    addEventListener(type, callback) {
      const listeners = this.listeners.get(type) ?? [];
      listeners.push(callback);
      this.listeners.set(type, listeners);
    },
    createElement() {
      const node = new NodeFixture();
      node.ownerDocument = document;
      return node;
    },
    querySelectorAll() { return []; },
  };
  const notice = new NodeFixture();
  const allow = new NodeFixture();
  const reject = new NodeFixture();
  const status = new NodeFixture();
  const footerOpener = new NodeFixture();
  const dismiss = new NodeFixture();
  for (const node of [document.activeElement, notice, allow, reject, status, footerOpener, dismiss]) {
    node.ownerDocument = document;
  }
  notice.hidden = true;
  notice.dataset.analyticsEnabled = enabled ? 'true' : 'false';
  notice.querySelector = (selector) => ({
    '[data-analytics-choice="allow"]': allow,
    '[data-analytics-choice="reject"]': reject,
    '[data-privacy-status]': status,
    '[data-dismiss-notice]': dismiss,
  })[selector] ?? null;

  const location = {
    href: `https://orgbots.theaecestudio.com/install/${hash}`,
    pathname: '/install/',
    search: '',
    hash,
    reloadCount: 0,
    reload() { this.reloadCount += 1; },
  };
  const history = {
    state: { existing: true },
    replaceState(state, _title, path) {
      const next = new URL(path, location.href);
      this.state = state;
      location.href = next.href;
      location.pathname = next.pathname;
      location.search = next.search;
      location.hash = next.hash;
    },
  };
  const window = {
    localStorage: local ?? storage(),
    sessionStorage: session,
    location,
    history,
    matchMedia: () => ({ matches: false }),
  };
  document.querySelector = (selector) => ({
    '.privacy-notice': notice,
    '[data-open-notice]': footerOpener,
  })[selector] ?? null;
  window.addEventListener = (type, callback) => {
    const listeners = window.listeners.get(type) ?? [];
    listeners.push(callback);
    window.listeners.set(type, listeners);
  };
  window.listeners = new Map();
  window.fire = (type, event = {}) => {
    for (const callback of window.listeners.get(type) ?? []) callback(event);
  };

  const context = vm.createContext({
    document,
    window,
    navigator: {},
    URL,
    Date,
    console,
  });
  vm.runInContext(source, context, { filename: 'website/site.js' });
  return { document, notice, allow, reject, status, footerOpener, dismiss, location, window, local: window.localStorage };
}

{
  const oldAcknowledgment = storage({ 'orgbots-privacy-notice-dismissed': 'true' });
  const page = boot({ local: oldAcknowledgment });
  assert.equal(page.notice.hidden, true, 'disabled analytics keeps the informational notice closed initially');
  assert.equal(page.document.activeElement.focusCount, 0, 'initial notice must not move focus');
  assert.equal(oldAcknowledgment.reads, 0, 'disabled analytics must not inspect consent storage');
  page.footerOpener.fire();
  assert.equal(page.notice.hidden, false, 'footer control opens informational notice');
  assert.equal(page.document.activeElement, page.dismiss, 'opening focuses the close control');
  page.dismiss.fire();
  assert.equal(page.notice.hidden, true, 'informational notice can be closed');
  assert.equal(page.document.activeElement, page.footerOpener, 'closing restores footer focus');
  assert.equal(oldAcknowledgment.reads, 0, 'notice controls must not read analytics consent');
  assert.equal(oldAcknowledgment.writes, 0, 'notice controls must not write analytics consent');
  assert.equal(page.document.head.scripts.length, 0, 'disabled analytics must never request the beacon');
}

{
  const page = boot({ enabled: true });
  page.allow.fire();
  assert.equal(page.notice.hidden, true);
  assert.equal(page.document.head.scripts.length, 1);
  const [beacon] = page.document.head.scripts;
  assert.equal(beacon.type, 'module');
  assert.equal(beacon.src, BEACON_SRC);
  assert.deepEqual(JSON.parse(beacon.getAttribute('data-cf-beacon')), { token: BEACON_TOKEN, spa: false });
  const saved = JSON.parse(page.window.localStorage.getItem(CONSENT_KEY));
  assert.equal(saved.choice, 'allow');
  assert.equal(saved.expiresAt - saved.savedAt, LIFETIME);
}

{
  const page = boot({ enabled: true, local: storage({ [CONSENT_KEY]: preference('allow', Date.now() - LIFETIME - 1000) }) });
  assert.equal(page.notice.hidden, false, 'expired allow must require a fresh choice');
  assert.equal(page.document.head.scripts.length, 0);
}

{
  const page = boot({ enabled: true, local: storage({}, { failGet: true, failSet: true }) });
  assert.equal(page.notice.hidden, false);
  assert.match(page.status.textContent, /could not be read/);
  assert.equal(page.document.head.scripts.length, 0);
  page.allow.fire();
  assert.match(page.status.textContent, /could not be saved/);
  assert.equal(page.document.head.scripts.length, 0);
}

{
  const priorAllow = storage({ [CONSENT_KEY]: preference('allow') }, { failSet: true });
  const sameTab = storage();
  const allowedPage = boot({ enabled: true, local: priorAllow, session: sameTab });
  assert.equal(allowedPage.document.head.scripts.length, 1);
  allowedPage.footerOpener.fire();
  assert.equal(allowedPage.notice.hidden, false);
  assert.equal(allowedPage.document.activeElement, allowedPage.allow, 'reopening preferences focuses the first choice');
  allowedPage.reject.fire();
  assert.equal(allowedPage.location.reloadCount, 1, 'loaded analytics must be stopped through a page reload');
  assert.match(allowedPage.location.hash, /^#orgbots-analytics-disabled=/);

  const reloadedPage = boot({ enabled: true, local: priorAllow, session: sameTab, hash: allowedPage.location.hash });
  assert.equal(reloadedPage.document.head.scripts.length, 0, 'reload marker must override a stale stored allow');
  assert.equal(reloadedPage.notice.hidden, false, 'unsaved rejection remains visible for another explicit choice');
  assert.match(reloadedPage.status.textContent, /previous rejection could not be saved/);
  assert.equal(reloadedPage.window.localStorage.getItem(CONSENT_KEY), priorAllow.values.get(CONSENT_KEY));
}

{
  const rejectedPage = boot({ enabled: true, local: storage({ [CONSENT_KEY]: preference('reject') }) });
  assert.equal(rejectedPage.document.head.scripts.length, 0);
  assert.equal(rejectedPage.notice.hidden, true);
  rejectedPage.footerOpener.fire();
  assert.equal(rejectedPage.document.activeElement, rejectedPage.allow);
  rejectedPage.allow.fire();
  assert.equal(rejectedPage.notice.hidden, true);
  assert.equal(rejectedPage.document.activeElement, rejectedPage.footerOpener, 'choice returns focus to the footer control');
  assert.equal(rejectedPage.document.head.scripts.length, 1, 'a later explicit allow starts analytics');
}

{
  const local = storage();
  const page = boot({ enabled: true, local });
  page.allow.fire();
  assert.equal(page.document.head.scripts.length, 1);
  local.removeItem(CONSENT_KEY);
  page.window.fire('storage', { key: CONSENT_KEY });
  assert.equal(page.location.reloadCount, 1, 'cross-tab consent removal reloads a page with an active beacon');
}

{
  const local = storage();
  const page = boot({ enabled: true, local });
  page.allow.fire();
  const expired = JSON.parse(local.getItem(CONSENT_KEY));
  expired.savedAt -= LIFETIME + 1;
  expired.expiresAt -= LIFETIME + 1;
  local.values.set(CONSENT_KEY, JSON.stringify(expired));
  page.window.fire('pageshow');
  assert.equal(page.location.reloadCount, 1, 'restored page reloads when consent has expired');
}

console.log('website analytics gate, consent storage, reload fallback, withdrawal, expiry, and focus checks passed');
