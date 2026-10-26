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
  const storageKey = 'orgbots-privacy-notice-dismissed';
  const dismissButton = notice.querySelector('[data-dismiss-notice]');
  let noticeOpener = null;
  const showNotice = (moveFocus = false) => {
    noticeOpener = moveFocus ? document.activeElement : null;
    notice.hidden = false;
    if (moveFocus) dismissButton.focus();
  };
  const hideNotice = () => {
    notice.hidden = true;
    noticeOpener?.focus({ preventScroll: true });
    try { window.localStorage.setItem(storageKey, 'true'); } catch { /* The notice remains dismissible for this visit. */ }
  };
  try { if (window.localStorage.getItem(storageKey) !== 'true') showNotice(); } catch { showNotice(); }
  dismissButton.addEventListener('click', hideNotice);
  document.querySelector('[data-open-notice]')?.addEventListener('click', () => showNotice(true));
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
