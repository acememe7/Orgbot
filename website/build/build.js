// The build log's interactions. The page reads fine without them: every card
// is in the HTML, and noscript.css opens them all.
(() => {
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ---- Counters ------------------------------------------------------------
  if (!reduced) {
    document.querySelectorAll('.bl-stats [data-count]').forEach((el) => {
      const target = Number(el.dataset.count);
      const start = performance.now();
      const step = (now) => {
        const t = Math.min(1, (now - start) / 1400);
        el.textContent = Math.round(target * (1 - Math.pow(1 - t, 3))).toLocaleString('en-GB');
        if (t < 1) requestAnimationFrame(step);
      };
      el.textContent = '0';
      requestAnimationFrame(step);
    });
  }

  // ---- Latest builds: switches on its own, or by hand ------------------------
  const carousel = document.querySelector('.latest');
  if (carousel) {
    const slides = [...carousel.querySelectorAll('.slide')];
    const tabs = [...carousel.querySelectorAll('.dot')];
    const toggle = carousel.querySelector('[data-latest="toggle"]');
    const viewport = carousel.querySelector('.latest-viewport');
    const interval = 7000;
    let index = 0;
    let elapsed = 0;
    let last = performance.now();
    // Paused by the reader, or for now (pointer or focus inside, off screen).
    let stopped = reduced;
    let held = false;
    let visible = true;

    const show = (next) => {
      index = (next + slides.length) % slides.length;
      elapsed = 0;
      slides.forEach((slide, i) => {
        const on = i === index;
        slide.classList.toggle('is-active', on);
        slide.hidden = !on;
      });
      tabs.forEach((tab, i) => {
        const on = i === index;
        tab.classList.toggle('is-active', on);
        tab.classList.toggle('is-done', i < index);
        tab.setAttribute('aria-selected', String(on));
        tab.tabIndex = on ? 0 : -1;
        tab.querySelector('.dot-bar span').style.transform = i < index ? 'scaleX(1)' : 'scaleX(0)';
      });
    };
    const setStopped = (value) => {
      stopped = value;
      toggle.dataset.paused = String(value);
      toggle.setAttribute('aria-label', value ? 'Start automatic switching' : 'Pause automatic switching');
      viewport.setAttribute('aria-live', value ? 'polite' : 'off');
    };
    const byHand = (next) => {
      setStopped(true);
      show(next);
    };
    carousel.querySelector('[data-latest="prev"]').addEventListener('click', () => byHand(index - 1));
    carousel.querySelector('[data-latest="next"]').addEventListener('click', () => byHand(index + 1));
    toggle.addEventListener('click', () => {
      setStopped(!stopped);
      last = performance.now();
    });
    tabs.forEach((tab, i) => {
      tab.addEventListener('click', () => byHand(i));
      tab.addEventListener('keydown', (event) => {
        const moves = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: slides.length - 1 };
        if (!(event.key in moves)) return;
        event.preventDefault();
        byHand(moves[event.key]);
        tabs[index].focus();
      });
    });
    carousel.addEventListener('pointerenter', () => { held = true; });
    carousel.addEventListener('pointerleave', () => { held = false; last = performance.now(); });
    carousel.addEventListener('focusin', () => { held = true; });
    carousel.addEventListener('focusout', (event) => {
      if (!carousel.contains(event.relatedTarget)) { held = false; last = performance.now(); }
    });
    new IntersectionObserver(([entry]) => { visible = entry.isIntersecting; last = performance.now(); }).observe(carousel);

    const tick = (now) => {
      const dt = now - last;
      last = now;
      if (!stopped && !held && visible && !document.hidden) {
        elapsed += dt;
        const bar = tabs[index].querySelector('.dot-bar span');
        bar.style.transform = `scaleX(${Math.min(1, elapsed / interval)})`;
        if (elapsed >= interval) show(index + 1);
      }
      requestAnimationFrame(tick);
    };
    setStopped(stopped);
    show(0);
    requestAnimationFrame(tick);
  }

  // ---- Every build: filters, search, days ------------------------------------
  const timeline = document.getElementById('timeline');
  if (timeline) {
    const builds = [...timeline.querySelectorAll('.build')];
    const groups = [...timeline.querySelectorAll('.day-group')];
    const chips = [...document.querySelectorAll('.chip')];
    const dayButtons = [...document.querySelectorAll('.day')];
    const days = document.querySelector('.days');
    const search = document.getElementById('build-search');
    const status = document.getElementById('filter-status');
    const empty = document.getElementById('no-match');
    const labels = Object.fromEntries(chips.map((chip) => [chip.dataset.kind, chip.firstChild.nodeType === 3 ? chip.firstChild.textContent : chip.childNodes[1].textContent]));
    const state = { kind: 'all', day: null, words: [] };

    const apply = () => {
      let shown = 0;
      builds.forEach((build) => {
        const ok = (state.kind === 'all' || build.dataset.kind === state.kind) &&
          (!state.day || build.dataset.day === state.day) &&
          state.words.every((word) => build.dataset.search.includes(word));
        build.classList.toggle('is-hidden', !ok);
        if (ok) shown += 1;
      });
      groups.forEach((group) => group.classList.toggle('is-hidden', !group.querySelector('.build:not(.is-hidden)')));
      const parts = [];
      if (state.kind !== 'all') parts.push(labels[state.kind].trim().toLowerCase());
      if (state.day) parts.push(`on ${dayButtons.find((b) => b.dataset.day === state.day).querySelector('.day-label').textContent}`);
      if (state.words.length) parts.push(`matching "${search.value.trim()}"`);
      status.textContent = shown === builds.length
        ? `Showing all ${builds.length} builds.`
        : `Showing ${shown} of ${builds.length} builds${parts.length ? `: ${parts.join(', ')}` : ''}.`;
      empty.hidden = shown !== 0;
      days.classList.toggle('is-filtered', Boolean(state.day));
    };

    chips.forEach((chip) => chip.addEventListener('click', () => {
      state.kind = chip.dataset.kind;
      chips.forEach((c) => {
        c.classList.toggle('is-on', c === chip);
        c.setAttribute('aria-pressed', String(c === chip));
      });
      apply();
    }));
    dayButtons.forEach((button) => button.addEventListener('click', () => {
      state.day = state.day === button.dataset.day ? null : button.dataset.day;
      dayButtons.forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.day === state.day)));
      apply();
    }));
    let pending = 0;
    search.addEventListener('input', () => {
      clearTimeout(pending);
      pending = setTimeout(() => {
        state.words = search.value.toLowerCase().split(/\s+/).filter(Boolean);
        apply();
      }, 120);
    });
    empty.querySelector('[data-clear]').addEventListener('click', () => {
      search.value = '';
      state.words = [];
      state.day = null;
      dayButtons.forEach((b) => b.setAttribute('aria-pressed', 'false'));
      chips[0].click();
    });

    timeline.addEventListener('click', (event) => {
      const button = event.target.closest('.build-toggle');
      if (!button) return;
      button.setAttribute('aria-expanded', String(button.getAttribute('aria-expanded') !== 'true'));
    });

    // A link to one build (#b-<commit>) opens it.
    const openFromHash = () => {
      const body = location.hash && document.getElementById(location.hash.slice(1));
      const build = body && body.closest('.build');
      if (!build) return;
      build.querySelector('.build-toggle').setAttribute('aria-expanded', 'true');
      build.classList.add('is-flash');
      setTimeout(() => build.classList.remove('is-flash'), 1600);
      build.scrollIntoView({ block: 'center', behavior: reduced ? 'auto' : 'smooth' });
    };
    window.addEventListener('hashchange', openFromHash);
    openFromHash();

    // The day chart grows in when it comes into view.
    const grow = () => dayButtons.forEach((button) => {
      const max = Number(button.dataset.max);
      button.querySelectorAll('.seg').forEach((seg) => { seg.style.height = `${(Number(seg.dataset.n) / max) * 100}%`; });
    });
    if (reduced) grow();
    else new IntersectionObserver(([entry], observer) => {
      if (entry.isIntersecting) { grow(); observer.disconnect(); }
    }, { threshold: 0.3 }).observe(days);
  }

  // ---- The room: loaded only when it is about to be seen ----------------------
  const stage = document.getElementById('room-stage');
  if (stage) {
    const fail = () => {
      stage.classList.add('is-failed');
      document.getElementById('room-fallback').hidden = false;
      document.getElementById('room-caption').textContent = 'The 3D view could not start in this browser.';
      const room = document.getElementById('room');
      room.classList.add('is-failed');
      room.querySelector('.room-steps').open = true;
    };
    stage.addEventListener('room:fail', fail);
    const load = () => {
      const script = document.createElement('script');
      script.src = stage.dataset.sceneSrc;
      script.async = true;
      script.addEventListener('error', fail);
      document.body.appendChild(script);
    };
    if ('IntersectionObserver' in window) {
      new IntersectionObserver(([entry], observer) => {
        if (entry.isIntersecting) { observer.disconnect(); load(); }
      }, { rootMargin: '600px 0px' }).observe(stage);
    } else {
      load();
    }
  }
})();
