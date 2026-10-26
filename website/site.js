document.documentElement.classList.add('js');
const menuButton = document.querySelector('.menu-toggle');
const nav = document.querySelector('.nav');
if (menuButton && nav) {
  const closeMenu = () => {
    menuButton.setAttribute('aria-expanded', 'false');
    nav.dataset.open = 'false';
  };
  menuButton.addEventListener('click', () => {
    const open = menuButton.getAttribute('aria-expanded') !== 'true';
    menuButton.setAttribute('aria-expanded', String(open));
    nav.dataset.open = String(open);
  });
  nav.addEventListener('click', (event) => {
    if (event.target instanceof HTMLAnchorElement && window.matchMedia('(max-width: 620px)').matches) closeMenu();
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && menuButton.getAttribute('aria-expanded') === 'true') {
      closeMenu();
      menuButton.focus();
    }
  });
}

const gallery = document.querySelector('[data-gallery]');
if (gallery) {
  const image = gallery.querySelector('[data-gallery-image]');
  const caption = gallery.querySelector('[data-gallery-caption]');
  const stage = gallery.querySelector('[data-gallery-stage]');
  const status = gallery.querySelector('[data-gallery-status]');
  const retry = gallery.querySelector('[data-gallery-retry]');
  const choices = [...gallery.querySelectorAll('[data-gallery-choice]')];
  let activeSource = image.getAttribute('src');
  let retryCount = 0;
  const showLoaded = () => { stage.dataset.state = 'loaded'; status.textContent = ''; retry.hidden = true; };
  const showFailed = () => { stage.dataset.state = 'error'; status.textContent = 'This screenshot could not be loaded.'; retry.hidden = false; };
  image.addEventListener('load', showLoaded);
  image.addEventListener('error', showFailed);
  if (image.complete) image.naturalWidth ? showLoaded() : showFailed();
  else status.textContent = 'Loading screenshot.';
  retry.addEventListener('click', () => {
    stage.dataset.state = 'loading';
    status.textContent = 'Loading screenshot.';
    retry.hidden = true;
    image.src = `${activeSource}?retry=${++retryCount}`;
  });
  choices.forEach((button) => {
    button.addEventListener('click', () => {
      activeSource = button.dataset.src;
      stage.dataset.state = 'loading';
      status.textContent = 'Loading screenshot.';
      retry.hidden = true;
      image.src = activeSource;
      image.alt = button.dataset.alt;
      caption.textContent = button.dataset.caption;
      gallery.querySelectorAll('[data-gallery-choice]').forEach((item) => item.setAttribute('aria-pressed', String(item === button)));
    });
  });
}

const notice = document.querySelector('.privacy-notice');
if (notice) {
  if (notice.dataset.analyticsEnabled !== 'true') {
    const dismissButton = notice.querySelector('[data-dismiss-notice]');
    const opener = document.querySelector('[data-open-notice]');
    dismissButton?.addEventListener('click', () => {
      notice.hidden = true;
      opener?.focus({ preventScroll: true });
    });
    opener?.addEventListener('click', () => {
      notice.hidden = false;
      dismissButton?.focus();
    });
  } else {
  const allowButton = notice.querySelector('[data-analytics-choice="allow"]');
  const rejectButton = notice.querySelector('[data-analytics-choice="reject"]');
  const status = notice.querySelector('[data-privacy-status]');
  const opener = document.querySelector('[data-open-notice]');
  const storageKey = 'orgbots-analytics-consent-v1';
  const sessionKey = 'orgbots-analytics-disabled-v1';
  const consentLifetime = 180 * 24 * 60 * 60 * 1000;
  const reloadMarkerPrefix = '#orgbots-analytics-disabled=';
  const beaconToken = '46be099b90994f12997c7c0b9019c0f0';
  let choiceOpener = null;
  let beaconLoaded = false;
  let pageBlocked = false;
  let currentPreference = null;

  const setStatus = (message) => { status.textContent = message; };
  const readPreference = () => {
    try {
      const raw = window.localStorage.getItem(storageKey);
      if (raw === null) return { preference: null, failed: false };
      const saved = JSON.parse(raw);
      if (!saved || saved.version !== 1 || !['allow', 'reject'].includes(saved.choice) ||
          !Number.isSafeInteger(saved.savedAt) || !Number.isSafeInteger(saved.expiresAt) ||
          saved.savedAt > Date.now() || saved.expiresAt - saved.savedAt !== consentLifetime ||
          saved.expiresAt <= Date.now()) {
        return { preference: null, failed: false };
      }
      return { preference: saved.choice, failed: false };
    } catch {
      return { preference: null, failed: true };
    }
  };
  const savePreference = (choice) => {
    const savedAt = Date.now();
    try {
      window.localStorage.setItem(storageKey, JSON.stringify({
        version: 1,
        choice,
        savedAt,
        expiresAt: savedAt + consentLifetime,
      }));
      return true;
    } catch {
      return false;
    }
  };
  const sessionDenied = () => {
    try { return window.sessionStorage.getItem(sessionKey) === 'true'; } catch { return false; }
  };
  const writeSessionDenial = () => {
    try {
      window.sessionStorage.setItem(sessionKey, 'true');
      return true;
    } catch {
      return false;
    }
  };
  const clearSessionDenial = () => {
    try {
      window.sessionStorage.removeItem(sessionKey);
      return true;
    } catch {
      return false;
    }
  };
  const savedHashFromMarker = () => {
    const hash = window.location.hash;
    if (!hash.startsWith(reloadMarkerPrefix)) return null;
    try { return decodeURIComponent(hash.slice(reloadMarkerPrefix.length)); } catch { return ''; }
  };
  const replaceCurrentUrl = (hash) => {
    const url = new URL(window.location.href);
    url.hash = hash;
    try {
      window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`);
      return true;
    } catch {
      return false;
    }
  };
  const setReloadMarker = () => {
    const existing = savedHashFromMarker();
    const previousHash = existing ?? window.location.hash;
    const marker = `${reloadMarkerPrefix}${encodeURIComponent(previousHash)}`;
    return replaceCurrentUrl(marker.slice(1)) && window.location.hash.startsWith(reloadMarkerPrefix);
  };
  const clearReloadMarker = () => {
    const previousHash = savedHashFromMarker();
    return previousHash === null || replaceCurrentUrl(previousHash);
  };
  const showNotice = (moveFocus = false) => {
    choiceOpener = moveFocus ? document.activeElement : null;
    notice.hidden = false;
    if (moveFocus) allowButton.focus();
  };
  const hideNotice = () => {
    notice.hidden = true;
    const returnTarget = choiceOpener;
    choiceOpener = null;
    returnTarget?.focus({ preventScroll: true });
  };
  const startBeacon = () => {
    if (beaconLoaded || pageBlocked) return;
    const script = document.createElement('script');
    script.type = 'module';
    script.src = 'https://static.cloudflareinsights.com/beacon.min.js';
    script.setAttribute('data-cf-beacon', JSON.stringify({ token: beaconToken, spa: false }));
    document.head.appendChild(script);
    beaconLoaded = true;
  };
  const choose = (choice) => {
    const saved = savePreference(choice);
    if (choice === 'reject') {
      pageBlocked = true;
      const sessionSaved = writeSessionDenial();
      if (beaconLoaded) {
        const markerSaved = setReloadMarker();
        if (!saved && !markerSaved && !sessionSaved) {
          setStatus('Your rejection could not be saved or safely applied. Close this tab to stop analytics.');
          return;
        }
        if (!saved) {
          setStatus('Your rejection could not be saved. Analytics will stay off for this tab, but this browser may ask again on a later visit.');
        }
        window.location.reload();
        return;
      }
      if (!saved) {
        setStatus('Your rejection could not be saved. Analytics is off for this visit; this browser may ask again later.');
        return;
      }
      setStatus('');
      hideNotice();
      return;
    }

    if (!saved) {
      setStatus(currentPreference === 'allow' && !pageBlocked
        ? 'Your choice could not be saved. Your existing saved consent remains in effect.'
        : 'Your choice could not be saved. Analytics stays off until your choice can be saved.');
      return;
    }
    if (!clearSessionDenial() || !clearReloadMarker()) {
      pageBlocked = true;
      setStatus('Your choice was saved, but the earlier rejection could not be cleared in this tab. Analytics remains off here.');
      return;
    }
    pageBlocked = false;
    currentPreference = 'allow';
    startBeacon();
    setStatus('');
    hideNotice();
  };

  const recheckConsent = () => {
    const result = readPreference();
    const validAllow = result.preference === 'allow' && !sessionDenied() && savedHashFromMarker() === null;
    if (!validAllow) {
      currentPreference = result.preference;
      pageBlocked = true;
      if (beaconLoaded) {
        window.location.reload();
      } else if (result.preference !== 'reject') {
        showNotice();
        setStatus(result.failed
          ? 'Your saved choice could not be read. Analytics is off until you choose.'
          : 'Your analytics choice expired or changed. Choose again to continue.');
      }
      return;
    }
    currentPreference = 'allow';
    if (!beaconLoaded) startBeacon();
  };

  if (allowButton && rejectButton && status) {
  allowButton.addEventListener('click', () => choose('allow'));
  rejectButton.addEventListener('click', () => choose('reject'));
  opener?.addEventListener('click', () => {
    setStatus('');
    showNotice(true);
  });

  const { preference, failed } = readPreference();
  currentPreference = preference;
  const reloadHash = savedHashFromMarker();
  const blockedInTab = sessionDenied();
  pageBlocked = reloadHash !== null || blockedInTab;
  if (pageBlocked) {
    const sessionSaved = writeSessionDenial();
    if (preference === 'reject') {
      clearReloadMarker();
    } else if (sessionSaved) {
      clearReloadMarker();
    } else {
      pageBlocked = true;
    }
    if (preference !== 'reject') {
      showNotice();
      setStatus('Your previous rejection could not be saved. Analytics is off in this tab; choose again to save your preference.');
    }
  } else if (preference === 'allow') {
    startBeacon();
  } else if (preference === 'reject') {
    notice.hidden = true;
  } else {
    showNotice();
    if (failed) setStatus('Your saved choice could not be read. Analytics is off until you choose.');
  }
  window.addEventListener('storage', (event) => {
    if (event.key === storageKey || event.key === null) recheckConsent();
  });
  window.addEventListener('pageshow', recheckConsent);
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') recheckConsent();
  });
  }
  }
}

document.querySelectorAll('[data-copy]').forEach((button) => {
  button.addEventListener('click', async () => {
    const status = button.parentElement.querySelector('[role="status"]');
    try {
      if (!navigator.clipboard) throw new Error('Clipboard API unavailable');
      await navigator.clipboard.writeText(button.dataset.copy);
      status.textContent = 'Command copied.';
    } catch (error) {
      status.textContent = error instanceof Error && error.message === 'Clipboard API unavailable'
        ? 'Clipboard is unavailable. Select and copy the command above.'
        : 'Copy failed. Select and copy the command above.';
    }
  });
});
