// How an OrgBots room runs, in 3D, for the public build log.
//
// An illustration with the live room's real roster: the bots, the shared
// board, each bot's private copy, the owner and the project. Bundled with
// three.js into website/build/scene.js (see README.md beside this file).
import {
  ACESFilmicToneMapping, BoxGeometry, BufferAttribute, BufferGeometry, CanvasTexture, CapsuleGeometry, CircleGeometry,
  Color, CylinderGeometry, DirectionalLight, DoubleSide, EdgesGeometry, Fog, Group, HemisphereLight, Line,
  LineBasicMaterial, LineLoop, LineSegments, MathUtils, Mesh, MeshBasicMaterial, MeshStandardMaterial,
  PCFShadowMap, PerspectiveCamera, PlaneGeometry, Points, PointsMaterial,
  QuadraticBezierCurve3, Raycaster, RingGeometry, Scene, SphereGeometry, SRGBColorSpace, TorusGeometry, Vector2,
  Vector3, WebGLRenderer,
} from 'three';

const stage = document.getElementById('room-stage');
const canvas = document.getElementById('room-canvas');
const labelsLayer = document.getElementById('room-labels');
const captionEl = document.getElementById('room-caption');
const logEl = document.getElementById('room-log');
const infoEl = document.getElementById('room-info');
const room = document.getElementById('room');
const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

const COLOR = {
  bg: 0x121b17, floor: 0x16211c, line: 0x2a3b33, board: 0x24332c, paper: 0xece8dc, body: 0xd9d3c3, dark: 0x22302a,
  idle: 0x6b7a72, work: 0x8fc7a9, wait: 0xd8ac5c, review: 0xb9a3d6, handoff: 0x8fb3e0, owner: 0xe09a7a, rest: 0x3f4d46,
};
const STATUS = {
  idle: { color: COLOR.idle, label: 'idle' },
  working: { color: COLOR.work, label: 'working' },
  waiting: { color: COLOR.wait, label: 'waiting on you' },
  reviewing: { color: COLOR.review, label: 'reviewing' },
  resting: { color: COLOR.rest, label: 'resting until something changes' },
};
const ROSTER = [
  { handle: 'grok-quick', roles: 'engineer, generalist', angle: 18 },
  { handle: 'mira', roles: 'engineer, frontend', angle: 54 },
  { handle: 'sol', roles: 'engineer, ops', angle: 90 },
  { handle: 'astra', roles: 'security, reviewer, coordinator', angle: 126 },
  { handle: 'opus', roles: 'architect, designer, product manager, coordinator', angle: 162 },
  { handle: 'grok', roles: 'support', angle: 198 },
  { handle: 'ollama-deepseek', roles: 'researcher, analyst', angle: 234 },
  { handle: 'ellis', roles: 'writer, editor, documenter', angle: 270 },
  { handle: 'ollama-glm', roles: 'QA', angle: 306 },
  { handle: 'deepseek-builder', roles: 'engineer, backend, data', angle: 342 },
];
const RING = 6.6;
const COPY_RING = 8.9;
const OWNER_AT = polar(30, 11.7);
const PROJECT_AT = polar(210, 11.9);
const GATE_AT = polar(210, 9.95);

function polar(degrees, radius, y = 0) {
  const a = MathUtils.degToRad(degrees);
  return new Vector3(Math.cos(a) * radius, y, Math.sin(a) * radius);
}

// ---- Time: tweens that pause, speed up, cancel ----------------------------
class Cancelled extends Error {}
const tweens = new Set();
let speed = 1;
let paused = false;
let instant = reduced;
const ease = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
const easeOut = (t) => 1 - Math.pow(1 - t, 3);
function tween(ms, update = () => {}, curve = ease) {
  return new Promise((resolve, reject) => {
    if (instant || ms <= 0) {
      update(1, 1);
      resolve();
      return;
    }
    tweens.add({ t: 0, ms, update, curve, resolve, reject });
  });
}
const sleep = (ms) => tween(ms);
function advance(dt) {
  if (paused) return;
  for (const tw of [...tweens]) {
    tw.t += dt * speed;
    const p = Math.min(1, tw.t / tw.ms);
    tw.update(tw.curve(p), p);
    if (p >= 1) {
      tweens.delete(tw);
      tw.resolve();
    }
  }
}
function cancelAll() {
  for (const tw of tweens) tw.reject(new Cancelled());
  tweens.clear();
}

function main() {
  let renderer;
  try {
    renderer = new WebGLRenderer({ canvas, antialias: true, powerPreference: 'high-performance' });
    if (!renderer.getContext()) throw new Error('no context');
  } catch {
    stage.dispatchEvent(new Event('room:fail'));
    return;
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.75));
  renderer.outputColorSpace = SRGBColorSpace;
  renderer.toneMapping = ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.15;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = PCFShadowMap;

  const scene = new Scene();
  scene.background = new Color(COLOR.bg);
  scene.fog = new Fog(COLOR.bg, 30, 64);
  const camera = new PerspectiveCamera(36, 1, 0.1, 200);

  scene.add(new HemisphereLight(0xe4ece4, 0x0b120f, 1.4));
  const sun = new DirectionalLight(0xfff1dc, 2.1);
  sun.position.set(9, 18, 11);
  sun.castShadow = true;
  sun.shadow.mapSize.set(1024, 1024);
  Object.assign(sun.shadow.camera, { left: -16, right: 16, top: 16, bottom: -16, near: 1, far: 50 });
  sun.shadow.bias = -0.0008;
  scene.add(sun);

  const std = (color, extra = {}) => new MeshStandardMaterial({ color, roughness: 0.78, metalness: 0.02, ...extra });
  const pickables = [];
  const pick = (mesh, target) => { mesh.userData.target = target; pickables.push(mesh); return mesh; };

  // ---- Floor and guides ---------------------------------------------------
  const floor = new Mesh(new CircleGeometry(34, 96), std(COLOR.floor, { roughness: 1 }));
  floor.rotation.x = -Math.PI / 2;
  floor.receiveShadow = true;
  scene.add(floor);
  const circle = (radius, color, opacity = 1) => {
    const pts = [];
    for (let i = 0; i < 128; i++) pts.push(polar((i / 128) * 360, radius, 0.012));
    return new LineLoop(new BufferGeometry().setFromPoints(pts), new LineBasicMaterial({ color, transparent: true, opacity }));
  };
  scene.add(circle(RING, COLOR.line, 0.9), circle(COPY_RING, COLOR.line, 0.55), circle(4.2, COLOR.line, 0.4));
  // Dust in the air, for depth. Still when motion is reduced.
  const moteCount = 180;
  const motePositions = new Float32Array(moteCount * 3);
  for (let i = 0; i < moteCount; i++) {
    const at = polar(Math.random() * 360, 3 + Math.random() * 15, 0.6 + Math.random() * 6);
    motePositions.set([at.x, at.y, at.z], i * 3);
  }
  const moteGeo = new BufferGeometry();
  moteGeo.setAttribute('position', new BufferAttribute(motePositions, 3));
  const motes = new Points(moteGeo, new PointsMaterial({ color: 0xcfd8cf, size: 0.05, transparent: true, opacity: 0.35, depthWrite: false }));
  scene.add(motes);

  // ---- The board ----------------------------------------------------------
  const board = new Group();
  const boardBase = new Mesh(new CylinderGeometry(2.7, 2.9, 0.32, 72), std(COLOR.board));
  boardBase.position.y = 0.16;
  boardBase.castShadow = boardBase.receiveShadow = true;
  board.add(pick(boardBase, 'board'));
  const boardTop = new Mesh(new CircleGeometry(2.62, 72), new MeshStandardMaterial({ map: boardTexture(), roughness: 0.9 }));
  boardTop.rotation.x = -Math.PI / 2;
  boardTop.position.y = 0.325;
  boardTop.receiveShadow = true;
  board.add(pick(boardTop, 'board'));
  scene.add(board);
  const COLUMN_X = { todo: -1.45, doing: 0, done: 1.45 };

  // ---- Bots, their copies -------------------------------------------------
  const bots = new Map();
  const botGeo = {
    base: new CylinderGeometry(0.5, 0.56, 0.12, 32),
    body: new CapsuleGeometry(0.32, 0.6, 6, 18),
    head: new SphereGeometry(0.27, 24, 16),
    crown: new TorusGeometry(0.34, 0.035, 8, 40),
    visor: new BoxGeometry(0.34, 0.08, 0.06),
    halo: new RingGeometry(0.64, 0.78, 48),
    copy: new BoxGeometry(1.0, 0.64, 1.0),
    plate: new BoxGeometry(0.62, 0.06, 0.62),
  };
  const copyEdges = new EdgesGeometry(botGeo.copy);
  for (const spec of ROSTER) {
    const home = polar(spec.angle, RING);
    const group = new Group();
    group.position.copy(home);
    group.lookAt(0, 0, 0);
    const statusMat = new MeshStandardMaterial({ color: COLOR.idle, emissive: COLOR.idle, emissiveIntensity: 0.9, roughness: 0.4 });
    const haloMat = new MeshBasicMaterial({ color: COLOR.idle, transparent: true, opacity: 0.75, side: DoubleSide, depthWrite: false });
    const bodyMat = std(COLOR.body, { roughness: 0.55 });
    const base = new Mesh(botGeo.base, std(COLOR.dark));
    base.position.y = 0.06;
    const body = new Mesh(botGeo.body, bodyMat);
    body.position.y = 0.76;
    const head = new Mesh(botGeo.head, bodyMat);
    head.position.y = 1.55;
    const crown = new Mesh(botGeo.crown, statusMat);
    crown.rotation.x = Math.PI / 2;
    crown.position.y = 1.55;
    const visor = new Mesh(botGeo.visor, statusMat);
    visor.position.set(0, 1.57, 0.25);
    const halo = new Mesh(botGeo.halo, haloMat);
    halo.rotation.x = -Math.PI / 2;
    halo.position.y = 0.02;
    [base, body, head].forEach((m) => { m.castShadow = true; pick(m, `bot:${spec.handle}`); });
    const figure = new Group();
    figure.add(base, body, head, crown, visor);
    group.add(figure, halo);
    scene.add(group);

    const copyAt = polar(spec.angle, COPY_RING, 0.33);
    const copyMat = new MeshStandardMaterial({ color: COLOR.work, transparent: true, opacity: 0.1, roughness: 0.3, depthWrite: false });
    const copy = new Group();
    copy.position.copy(copyAt);
    copy.lookAt(0, 0.33, 0);
    const box = new Mesh(botGeo.copy, copyMat);
    const edgeMat = new LineBasicMaterial({ color: 0x4c6a5d, transparent: true, opacity: 0.9 });
    copy.add(pick(box, `copy:${spec.handle}`), new LineSegments(copyEdges, edgeMat));
    scene.add(copy);
    const tether = new Line(new BufferGeometry().setFromPoints([polar(spec.angle, RING + 0.8, 0.02), polar(spec.angle, COPY_RING - 0.55, 0.02)]),
      new LineBasicMaterial({ color: COLOR.line, transparent: true, opacity: 0.8 }));
    scene.add(tether);

    bots.set(spec.handle, {
      ...spec, home, group, figure, statusMat, haloMat, bodyMat, copy, copyMat, edgeMat, plates: [],
      status: 'idle', task: null, phase: Math.random() * Math.PI * 2, working: 0,
      label: addLabel(`@${spec.handle}`, `bot:${spec.handle}`),
    });
  }

  // ---- The owner ----------------------------------------------------------
  const owner = new Group();
  owner.position.copy(OWNER_AT);
  owner.lookAt(0, 0, 0);
  const desk = new Mesh(new BoxGeometry(2.0, 0.72, 0.95), std(0x2e3b35));
  desk.position.set(0, 0.36, 0.55);
  desk.castShadow = desk.receiveShadow = true;
  const ownerBodyMat = std(0xd08f72, { roughness: 0.6 });
  const ownerBody = new Mesh(new CapsuleGeometry(0.36, 0.66, 6, 18), ownerBodyMat);
  ownerBody.position.set(0, 0.82, -0.2);
  const ownerHead = new Mesh(new SphereGeometry(0.29, 24, 16), std(COLOR.paper, { roughness: 0.5 }));
  ownerHead.position.set(0, 1.66, -0.2);
  const ownerHaloMat = new MeshBasicMaterial({ color: COLOR.owner, transparent: true, opacity: 0, side: DoubleSide, depthWrite: false });
  const ownerHalo = new Mesh(new RingGeometry(1.25, 1.42, 64), ownerHaloMat);
  ownerHalo.rotation.x = -Math.PI / 2;
  ownerHalo.position.y = 0.02;
  [desk, ownerBody, ownerHead].forEach((m) => { m.castShadow = true; pick(m, 'owner'); });
  owner.add(desk, ownerBody, ownerHead, ownerHalo);
  scene.add(owner);
  const ownerLabel = addLabel('You', 'owner', 'k-owner');
  // Cards wait above and in front of you, toward the middle of the room.
  const cardSpot = OWNER_AT.clone().lerp(new Vector3(), 0.34).setY(4.3);

  // ---- The project and its gate --------------------------------------------
  const project = new Group();
  project.position.copy(PROJECT_AT);
  project.lookAt(0, 0, 0);
  const plinth = new Mesh(new BoxGeometry(3.6, 0.24, 2.2), std(COLOR.dark));
  plinth.position.y = 0.12;
  plinth.receiveShadow = plinth.castShadow = true;
  project.add(pick(plinth, 'project'));
  scene.add(project);
  const layerGeo = new BoxGeometry(3.0, 0.3, 1.7);
  const layerEdges = new EdgesGeometry(layerGeo);
  const layers = [];
  const BASE_LAYERS = 6;
  const layerY = (i) => 0.24 + 0.15 + i * 0.31;
  function addLayer(fresh) {
    const mat = std(fresh ? COLOR.work : (layers.length % 2 ? 0xe2ddcf : 0xd8d2c2), { roughness: 0.6 });
    const layer = new Mesh(layerGeo, mat);
    layer.add(new LineSegments(layerEdges, new LineBasicMaterial({ color: 0x9a9585 })));
    layer.position.y = layerY(layers.length);
    layer.castShadow = layer.receiveShadow = true;
    project.add(pick(layer, 'project'));
    layers.push(layer);
    return layer;
  }
  for (let i = 0; i < BASE_LAYERS; i++) addLayer(false);
  const projectLabel = addLabel('Your project', 'project', 'k-place');

  const gate = new Group();
  gate.position.copy(GATE_AT);
  gate.lookAt(0, 0, 0);
  const gateMat = std(COLOR.wait, { emissive: COLOR.wait, emissiveIntensity: 0.15 });
  for (const x of [-1.25, 1.25]) {
    const post = new Mesh(new CylinderGeometry(0.07, 0.07, 1.8, 12), gateMat);
    post.position.set(x, 0.9, 0);
    post.castShadow = true;
    gate.add(pick(post, 'gate'));
  }
  const lintel = new Mesh(new BoxGeometry(2.7, 0.1, 0.1), gateMat);
  lintel.position.y = 1.8;
  gate.add(pick(lintel, 'gate'));
  scene.add(gate);
  const gateLabel = addLabel('Removal gate', 'gate', 'k-place');

  // ---- Labels -------------------------------------------------------------
  function addLabel(text, target, extra = '') {
    const el = document.createElement('span');
    el.className = `room-label ${extra}`.trim();
    el.textContent = text;
    labelsLayer.appendChild(el);
    return { el, target };
  }
  const labelAnchors = () => [
    ...[...bots.values()].map((b) => [b.label, b.group.position, 2.25]),
    [ownerLabel, OWNER_AT, 2.4],
    [projectLabel, PROJECT_AT, layerY(layers.length) + 0.55],
    [gateLabel, GATE_AT, 2.3],
  ];
  const hot = new Set();
  let picked = null;
  const narrow = () => stage.clientWidth < 640;
  const projected = new Vector3();
  function placeLabels() {
    const w = stage.clientWidth;
    const h = stage.clientHeight;
    for (const [mesh, el] of pinned) {
      projected.copy(mesh.position).project(camera);
      const x = MathUtils.clamp((projected.x * 0.5 + 0.5) * w, 150, w - 150);
      const y = Math.max(el.offsetHeight + 8, (-projected.y * 0.5 + 0.5) * h);
      el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) translate(-50%, -100%)`;
    }
    for (const [label, at, lift] of labelAnchors()) {
      projected.set(at.x, at.y + lift, at.z).project(camera);
      const behind = projected.z > 1;
      const x = (projected.x * 0.5 + 0.5) * w;
      const y = (-projected.y * 0.5 + 0.5) * h;
      label.el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) translate(-50%, -100%)`;
      const isBot = label.target.startsWith('bot:');
      const isHot = hot.has(label.target);
      const isPicked = picked === label.target;
      label.el.classList.toggle('is-hot', isHot);
      label.el.classList.toggle('is-picked', isPicked);
      label.el.classList.toggle('is-dim', isBot && hot.size > 0 && !isHot && !isPicked);
      const hide = behind || (label.target === 'gate' && !gateShown) || (narrow() && isBot && hot.size > 0 && !isHot && !isPicked);
      label.el.classList.toggle('is-hidden', hide);
    }
  }
  let gateShown = false;

  // ---- Moving things ------------------------------------------------------
  const dynamic = new Set();
  const addDynamic = (obj) => { scene.add(obj); dynamic.add(obj); return obj; };
  const dropDynamic = (obj) => { scene.remove(obj); dynamic.delete(obj); };
  const arc = (from, to, height) => {
    const mid = from.clone().lerp(to, 0.5);
    mid.y += height;
    return new QuadraticBezierCurve3(from.clone(), mid, to.clone());
  };
  const glow = (color, size = 0.13) => {
    const core = new Mesh(new SphereGeometry(size * 1.35, 16, 12), new MeshBasicMaterial({ color }));
    core.add(new Mesh(new SphereGeometry(size * 2.6, 16, 12), new MeshBasicMaterial({ color, transparent: true, opacity: 0.22, depthWrite: false })));
    return core;
  };
  function trailFor(color) {
    const n = 26;
    const positions = new Float32Array(n * 3);
    const colors = new Float32Array(n * 3);
    const geo = new BufferGeometry();
    geo.setAttribute('position', new BufferAttribute(positions, 3));
    geo.setAttribute('color', new BufferAttribute(colors, 3));
    const line = new Line(geo, new LineBasicMaterial({ vertexColors: true, transparent: true, opacity: 0.9 }));
    line.frustumCulled = false;
    const head = new Color(color);
    const tail = new Color(COLOR.bg);
    const history = [];
    return {
      line,
      push(p) {
        history.unshift(p.clone());
        if (history.length > n) history.pop();
        for (let i = 0; i < n; i++) {
          const q = history[Math.min(i, history.length - 1)];
          positions.set([q.x, q.y, q.z], i * 3);
          const c = head.clone().lerp(tail, i / (n - 1));
          colors.set([c.r, c.g, c.b], i * 3);
        }
        geo.attributes.position.needsUpdate = true;
        geo.attributes.color.needsUpdate = true;
      },
    };
  }
  async function fly(mesh, curve, ms, color) {
    const trail = color === undefined ? null : trailFor(color);
    if (trail) addDynamic(trail.line);
    if (!dynamic.has(mesh)) addDynamic(mesh);
    try {
      await tween(ms, (k) => {
        mesh.position.copy(curve.getPoint(k));
        trail?.push(mesh.position);
      });
    } finally {
      if (trail) dropDynamic(trail.line);
    }
  }
  function burst(at, color, radius = 1.2) {
    const ring = addDynamic(new Mesh(new RingGeometry(0.2, 0.28, 48), new MeshBasicMaterial({ color, transparent: true, opacity: 0.9, side: DoubleSide, depthWrite: false })));
    ring.rotation.x = -Math.PI / 2;
    ring.position.copy(at);
    return tween(900, (k) => {
      ring.scale.setScalar(1 + k * radius * 4);
      ring.material.opacity = 0.9 * (1 - k);
    }, easeOut).finally(() => dropDynamic(ring));
  }

  // ---- Where the camera leans ---------------------------------------------
  const lean = new Vector3();
  const leanTarget = new Vector3();
  let leanZoom = 1;
  let leanZoomTarget = 1;
  function focusOn(point, amount = 0.3) {
    if (!point || reduced || portrait()) {
      leanTarget.set(0, 0, 0);
      leanZoomTarget = 1;
      return;
    }
    leanTarget.copy(point).multiplyScalar(amount).setY(0);
    leanZoomTarget = 0.88;
  }

  // ---- Status -------------------------------------------------------------
  const castDots = new Map([...document.querySelectorAll('#room-cast [data-target]')].map((b) => [b.dataset.target, b.querySelector('.cast-dot')]));
  function setStatus(bot, status) {
    bot.status = status;
    const color = new Color(STATUS[status].color);
    bot.statusMat.color.copy(color);
    bot.statusMat.emissive.copy(color);
    bot.statusMat.emissiveIntensity = status === 'resting' ? 0.15 : 0.9;
    bot.haloMat.color.copy(color);
    bot.haloMat.opacity = status === 'resting' ? 0.25 : 0.75;
    bot.bodyMat.color.set(status === 'resting' ? 0x9c988c : COLOR.body);
    const dot = castDots.get(`bot:${bot.handle}`);
    if (dot) dot.className = `cast-dot${status === 'idle' || status === 'resting' ? '' : ` is-${status}`}`;
    refreshInfo();
  }
  const B = (handle) => bots.get(handle);

  // ---- Tasks on the board -------------------------------------------------
  const tiles = [];
  const tileGeo = new BoxGeometry(0.72, 0.1, 0.46);
  const tileColor = { todo: COLOR.paper, doing: COLOR.work, done: 0x7f8f87 };
  function slotFor(tile) {
    const same = tiles.filter((t) => t.state === tile.state);
    const i = same.indexOf(tile);
    return new Vector3(COLUMN_X[tile.state], 0.385, -1.05 + i * 0.6);
  }
  function relayout(ms = 600) {
    return Promise.all(tiles.map((tile) => {
      const from = tile.mesh.position.clone();
      const to = slotFor(tile);
      tile.mesh.material.color.set(tileColor[tile.state]);
      return tween(ms, (k) => tile.mesh.position.lerpVectors(from, to, k));
    }));
  }
  async function newTasks(titles) {
    const made = titles.map((title) => {
      const mesh = new Mesh(tileGeo, std(COLOR.paper, { roughness: 0.6 }));
      mesh.castShadow = true;
      mesh.position.set(0, -0.2, 0);
      board.add(pick(mesh, 'board'));
      dynamic.add(mesh);
      const tile = { title, state: 'todo', mesh, holder: null };
      tiles.push(tile);
      return tile;
    });
    for (const tile of made) {
      const to = slotFor(tile);
      const curve = arc(new Vector3(0, 0.3, 0), to, 1.4);
      await tween(520, (k) => tile.mesh.position.copy(curve.getPoint(k)), easeOut);
    }
    refreshInfo();
    return made;
  }
  const tileAt = (title) => tiles.find((t) => t.title === title);

  // ---- Bot actions --------------------------------------------------------
  async function walk(bot, to, ms = 1100) {
    const from = bot.group.position.clone();
    const target = to.clone();
    const facing = target.clone().sub(from);
    if (facing.lengthSq() > 0.01) {
      const look = bot.group.position.clone().add(facing);
      bot.group.lookAt(look.x, 0, look.z);
    }
    await tween(ms, (k) => bot.group.position.lerpVectors(from, target, k));
    bot.group.lookAt(0, 0, 0);
  }
  const nearBoard = (bot) => polar(bot.angle, 3.55);
  const home = (bot, ms) => walk(bot, bot.home, ms);

  async function claim(bot, title) {
    hot.add(`bot:${bot.handle}`);
    const tile = tileAt(title);
    await walk(bot, nearBoard(bot));
    tile.state = 'doing';
    tile.holder = bot.handle;
    bot.task = title;
    setStatus(bot, 'working');
    await Promise.all([relayout(500), burst(bot.group.position.clone().setY(0.03), COLOR.work, 0.4)]);
    await home(bot);
  }
  async function work(bot, commits) {
    setStatus(bot, 'working');
    hot.add(`bot:${bot.handle}`);
    bot.working = 1;
    for (let i = 0; i < commits; i++) {
      await sleep(700);
      const plate = new Mesh(botGeo.plate, std(COLOR.work, { emissive: COLOR.work, emissiveIntensity: 0.25 }));
      plate.position.set(0, 0.9, 0);
      bot.copy.add(plate);
      bot.plates.push(plate);
      const y = -0.28 + bot.plates.length * 0.08;
      await tween(420, (k) => { plate.position.y = MathUtils.lerp(0.9, y, k); }, easeOut);
    }
    bot.working = 0;
  }
  async function handoff(from, to, { busyFor = 0, color = COLOR.handoff } = {}) {
    hot.add(`bot:${from.handle}`);
    hot.add(`bot:${to.handle}`);
    const start = from.group.position.clone().setY(1.6);
    const end = to.group.position.clone().setY(2.35);
    const packet = glow(color, 0.15);
    await fly(packet, arc(start, end, 2.6), 1500, color);
    if (busyFor) {
      const ring = addDynamic(new Mesh(new TorusGeometry(0.32, 0.03, 8, 40), new MeshBasicMaterial({ color: COLOR.wait })));
      ring.position.copy(end);
      await tween(busyFor, (k) => {
        ring.rotation.x = k * 12;
        ring.rotation.y = k * 7;
        packet.position.y = end.y + Math.sin(k * Math.PI * 6) * 0.08;
      }, (t) => t);
      dropDynamic(ring);
    }
    await tween(380, (k) => { packet.position.y = MathUtils.lerp(end.y, 1.55, k); packet.scale.setScalar(1 - k); });
    dropDynamic(packet);
    await burst(to.group.position.clone().setY(0.03), color, 0.45);
  }

  const cardGeo = new PlaneGeometry(2.3, 1.44);
  const billboards = new Set();
  // A card that arrives becomes HTML pinned to its spot, so its words stay sharp.
  const pinned = new Map();
  function pinCard(mesh, card) {
    const el = document.createElement('div');
    el.className = 'room-card';
    const who = document.createElement('p');
    who.className = 'room-card-who';
    who.textContent = card.who;
    const title = document.createElement('p');
    title.className = 'room-card-title';
    title.textContent = card.title;
    el.append(who, title);
    for (const line of card.lines) {
      const p = document.createElement('p');
      p.textContent = line;
      el.appendChild(p);
    }
    const choices = document.createElement('div');
    choices.className = 'room-card-choices';
    for (const choice of card.choices) {
      const span = document.createElement('span');
      span.textContent = choice;
      choices.appendChild(span);
    }
    el.appendChild(choices);
    labelsLayer.appendChild(el);
    pinned.set(mesh, el);
  }
  function unpinCard(mesh) {
    pinned.get(mesh)?.remove();
    pinned.delete(mesh);
  }
  async function ask(bot, card, { lean = true } = {}) {
    hot.add(`bot:${bot.handle}`);
    hot.add('owner');
    setStatus(bot, 'waiting');
    const mesh = new Mesh(cardGeo, new MeshBasicMaterial({ map: cardTexture(card, null), transparent: true, side: DoubleSide }));
    mesh.scale.setScalar(0.3);
    billboards.add(mesh);
    if (lean) focusOn(OWNER_AT, 0.32);
    const start = bot.group.position.clone().setY(1.9);
    const growing = tween(1400, (k) => mesh.scale.setScalar(0.3 + 1.6 * k));
    await Promise.all([fly(mesh, arc(start, cardSpot, 3.2), 1400, COLOR.wait), growing]);
    mesh.visible = false;
    pinCard(mesh, card);
    ownerHaloMat.opacity = 0.85;
    waitingCards += 1;
    refreshInfo();
    return mesh;
  }
  async function answer(mesh, card, choice, bot, { color = COLOR.wait } = {}) {
    await sleep(900);
    mesh.material.map.dispose();
    mesh.material.map = cardTexture(card, choice);
    pinned.get(mesh)?.querySelectorAll('.room-card-choices span')[choice]?.classList.add('is-chosen');
    await burst(OWNER_AT.clone().setY(0.03), COLOR.owner, 0.6);
    const remove = () => {
      unpinCard(mesh);
      billboards.delete(mesh);
      dropDynamic(mesh);
    };
    if (stepping) {
      // One step at a time: the answer stays in view until the next step.
      onNextStep.push(remove);
    } else {
      await sleep(1300);
      pinned.get(mesh)?.classList.add('is-leaving');
      await sleep(350);
      remove();
    }
    focusOn(null);
    waitingCards = Math.max(0, waitingCards - 1);
    ownerHaloMat.opacity = waitingCards ? 0.85 : 0;
    refreshInfo();
    if (!bot) return;
    const key = new Group();
    const ring = new Mesh(new TorusGeometry(0.12, 0.035, 8, 24), new MeshBasicMaterial({ color }));
    const stem = new Mesh(new BoxGeometry(0.05, 0.05, 0.26), new MeshBasicMaterial({ color }));
    stem.position.z = 0.18;
    key.add(ring, stem);
    await fly(key, arc(cardSpot.clone().setY(2.2), bot.group.position.clone().setY(1.7), 2.2), 1300, color);
    dropDynamic(key);
    setStatus(bot, 'working');
    await burst(bot.group.position.clone().setY(0.03), color, 0.45);
  }
  let waitingCards = 0;

  async function review(reviewer, author, approve) {
    hot.add(`bot:${reviewer.handle}`);
    hot.add(`bot:${author.handle}`);
    setStatus(reviewer, 'reviewing');
    const side = author.copy.position.clone().lerp(new Vector3(0, 0, 0), 0.22);
    side.y = 0;
    await walk(reviewer, side, 1400);
    const beam = addDynamic(new Line(new BufferGeometry().setFromPoints([new Vector3(), new Vector3()]),
      new LineBasicMaterial({ color: COLOR.review, transparent: true, opacity: 0.9 })));
    const from = reviewer.group.position.clone().setY(1.55);
    await tween(1500, (k) => {
      const sweep = Math.sin(k * Math.PI * 4) * 0.35;
      const to = author.copy.position.clone().add(new Vector3(sweep, Math.cos(k * Math.PI * 3) * 0.2, 0));
      beam.geometry.setFromPoints([from, to]);
      beam.material.opacity = 0.4 + 0.5 * Math.abs(Math.sin(k * Math.PI * 6));
    }, (t) => t);
    dropDynamic(beam);
    await burst(author.copy.position.clone().setY(0.04), approve ? COLOR.work : COLOR.wait, 0.7);
    author.edgeMat.color.set(approve ? COLOR.work : COLOR.wait);
    await home(reviewer, 1200);
    setStatus(reviewer, 'idle');
  }
  async function land(bot, label = true) {
    hot.add(`bot:${bot.handle}`);
    hot.add('project');
    const plates = [...bot.plates];
    bot.plates = [];
    for (const plate of plates) {
      const from = new Vector3();
      plate.getWorldPosition(from);
      bot.copy.remove(plate);
      const layer = addLayer(true);
      const local = layer.position.clone();
      const target = local.clone().applyMatrix4(project.matrixWorld);
      layer.visible = false;
      plate.position.copy(from);
      await fly(plate, arc(from, target, 3.2), 1150, COLOR.work);
      dropDynamic(plate);
      layer.visible = true;
      tween(1600, (k) => layer.material.color.lerpColors(new Color(COLOR.work), new Color(0xe2ddcf), k));
    }
    bot.edgeMat.color.set(0x4c6a5d);
    if (label) await burst(PROJECT_AT.clone().setY(0.03), COLOR.paper, 0.9);
    landed += plates.length;
    refreshInfo();
  }
  let landed = 0;

  // ---- Removals: files that wait at the gate ------------------------------
  let swarm = null;
  async function removals(bot, count) {
    hot.add(`bot:${bot.handle}`);
    gateShown = true;
    const n = Math.min(count, 220);
    const positions = new Float32Array(n * 3);
    const geo = new BufferGeometry();
    geo.setAttribute('position', new BufferAttribute(positions, 3));
    const points = addDynamic(new Points(geo, new PointsMaterial({ color: COLOR.wait, size: 0.09, transparent: true, opacity: 0.95, depthWrite: false })));
    const start = bot.copy.position.clone();
    const target = GATE_AT.clone().setY(0.9).add(polar(210, -0.9));
    const seeds = Array.from({ length: n }, () => [Math.random(), Math.random() * Math.PI * 2, Math.random() * 0.6, Math.random() * 0.9 + 0.2]);
    swarm = { points, geo, positions, seeds, target, n, kept: n };
    await tween(2600, (k) => {
      for (let i = 0; i < n; i++) {
        const [delay, angle, spread, height] = seeds[i];
        const t = MathUtils.clamp((k - delay * 0.4) / 0.6, 0, 1);
        const curve = arc(start, target, 3 + height);
        const p = curve.getPoint(ease(t));
        positions.set([p.x + Math.cos(angle) * spread * t, p.y + Math.sin(angle * 2) * spread * 0.5 * t, p.z + Math.sin(angle) * spread * t], i * 3);
      }
      geo.attributes.position.needsUpdate = true;
    }, (t) => t);
  }
  async function approveSome(keep) {
    if (!swarm) return;
    const { positions, geo, n, seeds } = swarm;
    const keepFrom = Math.round(n * (1 - keep));
    const starts = Array.from({ length: n }, (_, i) => new Vector3(positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]));
    const through = PROJECT_AT.clone().setY(0.6);
    await tween(2200, (k) => {
      for (let i = 0; i < keepFrom; i++) {
        const t = MathUtils.clamp((k - seeds[i][0] * 0.3) / 0.7, 0, 1);
        const p = starts[i].clone().lerp(through, ease(t));
        p.y -= t * t * 1.6;
        positions.set([p.x, p.y, p.z], i * 3);
      }
      geo.attributes.position.needsUpdate = true;
    }, (t) => t);
    for (let i = 0; i < keepFrom; i++) positions.set([0, -50, 0], i * 3);
    geo.attributes.position.needsUpdate = true;
    swarm.kept = n - keepFrom;
  }

  // ---- Caption, log, info -------------------------------------------------
  let clock = 14 * 60 + 2;
  const clockText = () => `${String(Math.floor(clock / 60) % 24).padStart(2, '0')}:${String(clock % 60).padStart(2, '0')}`;
  function caption(text) {
    captionEl.textContent = '';
    text.split(/(@[a-z-]+)/g).forEach((part) => {
      if (/^@[a-z-]+$/.test(part)) {
        const b = document.createElement('b');
        b.textContent = part;
        captionEl.appendChild(b);
      } else if (part) captionEl.appendChild(document.createTextNode(part));
    });
  }
  function log(type, text, minutes = 1) {
    clock += minutes;
    logEl.querySelector('.log-empty')?.remove();
    const li = document.createElement('li');
    li.className = `t-${type}`;
    const time = document.createElement('time');
    time.textContent = clockText();
    const dot = document.createElement('i');
    const span = document.createElement('span');
    span.textContent = text;
    li.append(time, dot, span);
    logEl.prepend(li);
    while (logEl.children.length > 7) logEl.lastElementChild.remove();
  }
  function clearLog() {
    logEl.textContent = '';
    const li = document.createElement('li');
    li.className = 'log-empty';
    li.textContent = 'Nothing yet in this run.';
    logEl.appendChild(li);
  }

  const INFO = {
    owner: () => ({
      name: 'You', role: 'the owner',
      text: 'What a bot cannot decide alone comes to you: sign-ins, pushes, risky commands and removals. Each card says why, what will run and what happens if you say no, and a yes lasts for one task.',
      now: waitingCards ? `${waitingCards} ${waitingCards === 1 ? 'question waits' : 'questions wait'} for you.` : 'Nothing waits for you.',
    }),
    project: () => ({
      name: 'Your project', role: 'the real repository',
      text: 'Bots never write here directly. Approved commits land on your branch, and files a bot deleted wait at the gate until you review them.',
      now: landed ? `${landed} ${landed === 1 ? 'commit' : 'commits'} landed in this run.` : 'Nothing has landed in this run yet.',
    }),
    board: () => {
      const n = (s) => tiles.filter((t) => t.state === s).length;
      return {
        name: 'The board', role: 'shared by every bot',
        text: 'Every task, who holds it and what it waits on. A bot sees what others added during its turn before it plans more, so work is not done twice.',
        now: tiles.length ? `${n('todo')} to do, ${n('doing')} in progress, ${n('done')} done.` : 'No tasks yet.',
      };
    },
    gate: () => ({
      name: 'The removal gate', role: 'deletions wait here',
      text: 'Files a bot deletes in its copy stop here. You see each folder with its size, whether git tracks it and when it last changed, and approve all of it or only some folders.',
      now: swarm ? (swarm.kept === swarm.n ? 'All 562 removals are waiting.' : '22 removals are still waiting.') : 'Nothing is waiting.',
    }),
  };
  function infoFor(target) {
    if (INFO[target]) return INFO[target]();
    const [kind, handle] = target.split(':');
    const b = bots.get(handle);
    if (!b) return null;
    if (kind === 'copy') {
      return {
        name: `@${handle}'s copy`, role: 'a private working copy',
        text: `A full copy of your project where @${handle} edits, builds and commits. It keeps its bot's name even if the bot leaves the room.`,
        now: b.plates.length ? `${b.plates.length} ${b.plates.length === 1 ? 'commit' : 'commits'} not landed yet.` : 'Nothing waiting to land.',
      };
    }
    return {
      name: `@${handle}`, role: b.roles,
      text: `Works in its own copy of your project. Its changes reach your project only by landing, after a review, and anything risky waits for your answer.`,
      now: `Now ${STATUS[b.status].label}${b.task && b.status !== 'resting' ? ` on "${b.task}"` : ''}.`,
    };
  }
  function refreshInfo() {
    if (!picked) return;
    const info = infoFor(picked);
    if (!info) return;
    infoEl.textContent = '';
    const name = document.createElement('p');
    name.className = 'info-name';
    name.textContent = info.name;
    const role = document.createElement('p');
    role.className = 'info-role';
    role.textContent = info.role;
    const text = document.createElement('p');
    text.textContent = info.text;
    const now = document.createElement('p');
    now.className = 'info-now';
    now.textContent = info.now;
    infoEl.append(name, role, text, now);
  }
  const castButtons = [...document.querySelectorAll('#room-cast [data-target]')];
  function select(target) {
    picked = target;
    castButtons.forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.target === target.replace(/^copy:/, 'bot:'))));
    refreshInfo();
  }
  castButtons.forEach((b) => b.addEventListener('click', () => select(b.dataset.target)));

  // ---- Scenarios -----------------------------------------------------------
  const GH_CARD = { who: '@sol asks', title: 'Use GitHub CLI for "Settings toggle"', lines: ['Why: push the branch for review', 'Will run: git push origin settings-toggle', 'If denied: hands you the command to run'], choices: ['Allow for this task', 'Deny'] };
  const LAND_CARD = { who: '@sol asks', title: 'Land 2 commits on main', lines: ['Rated risky', 'Recommended: Land these commits.', '@astra approved this work in review.'], choices: ['Land these commits', "Don't land them"] };
  const SIGNIN_CARD = { who: '@mira asks', title: 'A new sign-in: Cloudflare Wrangler', lines: ['Why: publish the preview build', 'Not on your list yet', 'If denied: hands you the command to run'], choices: ['Add it and allow for this task', 'Deny'] };
  const REMOVAL_CARD = { who: "@mira's copy", title: '562 removals to review', lines: ['logs/  540 files, not in git', '.freeze-probe/  22 files', 'changed 12 min ago, may be in use'], choices: ['Move 540 files to Trash', 'Choose folders'] };

  const SCENARIOS = {
    async run() {
      caption('You post a goal. No bot is named, so it reaches one coordinator, @opus.');
      log('owner', 'You: add a dark mode to settings', 0);
      hot.add('owner');
      const note = glow(COLOR.owner, 0.14);
      await fly(note, arc(OWNER_AT.clone().setY(1.7), B('opus').group.position.clone().setY(1.7), 3), 1700, COLOR.owner);
      dropDynamic(note);
      hot.clear();
      setStatus(B('opus'), 'working');
      await step();
      caption('@opus splits it into three tasks on the shared board.');
      hot.add('bot:opus');
      await walk(B('opus'), nearBoard(B('opus')));
      await newTasks(['Theme tokens', 'Settings toggle', 'Docs']);
      log('work', '@opus created 3 tasks', 2);
      await home(B('opus'));
      setStatus(B('opus'), 'idle');
      hot.clear();
      await step();
      caption('Bots claim the tasks that match their roles.');
      await Promise.all([claim(B('mira'), 'Theme tokens'), claim(B('sol'), 'Settings toggle'), claim(B('ellis'), 'Docs')]);
      log('work', '@mira claimed "Theme tokens"', 1);
      log('work', '@sol claimed "Settings toggle"', 0);
      log('work', '@ellis claimed "Docs"', 0);
      await step();
      caption('Each one works and commits inside its own copy. Your project is untouched.');
      await Promise.all([work(B('mira'), 2), work(B('sol'), 2), work(B('ellis'), 1)]);
      log('work', '@sol committed 2 changes in its copy', 6);
      await step();
      hot.clear();
      caption('@sol offers @mira an assist. @mira is busy, so the offer waits for it instead of expiring.');
      log('handoff', '@sol offered @mira an assist', 2);
      await handoff(B('sol'), B('mira'), { busyFor: 2600 });
      log('handoff', '@mira accepted when its turn ended', 3);
      await step();
      hot.clear();
      caption('@sol needs a sign-in it does not have, so it asks you: why, what it will run, and what happens if you say no.');
      log('ask', '@sol asked to use GitHub CLI for one task', 1);
      const card = await ask(B('sol'), GH_CARD);
      await step();
      caption('You allow it for this task only. When the task ends, the loan ends.');
      log('owner', 'You allowed GitHub CLI for "Settings toggle"', 2);
      await answer(card, GH_CARD, 0, B('sol'));
      await step();
      hot.clear();
      caption('@astra reviews the work before anything lands.');
      log('review', '@astra is reviewing "Settings toggle"', 2);
      await review(B('astra'), B('sol'), true);
      log('review', '@astra approved "Settings toggle"', 4);
      await step();
      hot.clear();
      caption('The approved commits land on your branch. Nothing else reaches your project.');
      await land(B('sol'));
      tileAt('Settings toggle').state = 'done';
      await relayout();
      log('land', '@sol landed 2 commits on main', 1);
      await step();
      hot.clear();
      caption('@mira and @ellis go through the same review, then land.');
      await Promise.all([land(B('mira'), false), land(B('ellis'), false)]);
      tileAt('Theme tokens').state = 'done';
      tileAt('Docs').state = 'done';
      await relayout();
      log('land', '@mira and @ellis landed 3 commits', 9);
      await step();
      hot.clear();
      caption('Nothing needs doing, so the bots rest. Their check-ins fold into one line.');
      [...bots.values()].forEach((b) => { b.task = null; setStatus(b, 'resting'); });
      log('work', '3 bots checked in. Nothing needs you.', 5);
      await sleep(3200);
    },
    async handoff() {
      caption('@opus hands @grok-quick a task. It is free, so it takes it at once.');
      log('handoff', '@opus handed "Fix the flaky test" to @grok-quick', 0);
      await handoff(B('opus'), B('grok-quick'));
      B('grok-quick').task = 'Fix the flaky test';
      setStatus(B('grok-quick'), 'working');
      log('handoff', '@grok-quick accepted', 1);
      await step();
      hot.clear();
      B('ollama-glm').task = 'Accessibility pass';
      setStatus(B('ollama-glm'), 'working');
      caption('@deepseek-builder offers @ollama-glm a test run while @ollama-glm is in the middle of a turn.');
      log('handoff', '@deepseek-builder offered @ollama-glm a test run', 2);
      await handoff(B('deepseek-builder'), B('ollama-glm'), { busyFor: 3600 });
      await step();
      caption('The offer waited instead of expiring. A paused room keeps the whole timeout too.');
      log('handoff', '@ollama-glm accepted when its turn ended', 4);
      B('ollama-glm').task = 'Run the API tests';
      refreshInfo();
      await work(B('ollama-glm'), 1);
      await step();
      caption('It sends the results back to @deepseek-builder, who carries on.');
      await handoff(B('ollama-glm'), B('deepseek-builder'), { color: COLOR.work });
      setStatus(B('deepseek-builder'), 'working');
      log('handoff', '@ollama-glm reported back: 41 passed', 6);
      await sleep(2600);
    },
    async ask() {
      await quietSetup();
      caption('@sol has two reviewed commits and asks to land them on main.');
      log('ask', '@sol asked to land 2 commits on main', 0);
      const card = await ask(B('sol'), LAND_CARD);
      await step();
      caption('The card says what each answer does and what the room recommends, with its reason.');
      await sleep(2400);
      await step();
      caption('You land them. Exactly these commits merge; anything newer needs asking again.');
      log('owner', 'You chose: Land these commits', 2);
      await answer(card, LAND_CARD, 0, B('sol'), { color: COLOR.work });
      await land(B('sol'));
      log('land', '@sol landed 2 commits on main', 1);
      await step();
      hot.clear();
      caption('@mira asks for a sign-in that is not on your list yet. Only you can add one.');
      log('ask', '@mira asked for a new sign-in: Cloudflare Wrangler', 4);
      const second = await ask(B('mira'), SIGNIN_CARD);
      await step();
      caption('You add it and allow it for this task. When the task ends, the loan ends.');
      log('owner', 'You chose: Add it and allow for this task', 2);
      await answer(second, SIGNIN_CARD, 0, B('mira'));
      await sleep(2400);
    },
    async land() {
      await quietSetup(3);
      caption('@sol finishes a change and asks @astra for a review.');
      log('review', '@sol asked @astra to review "Settings toggle"', 0);
      await handoff(B('sol'), B('astra'), { color: COLOR.review });
      await step();
      hot.clear();
      caption('@astra finds a problem and sends it back with the reason.');
      await review(B('astra'), B('sol'), false);
      log('review', '@astra asked for changes: the toggle forgets its state', 6);
      await step();
      hot.clear();
      caption('@sol fixes it in its copy.');
      await work(B('sol'), 1);
      log('work', '@sol committed the fix', 9);
      await step();
      hot.clear();
      caption('Second review: approved.');
      await review(B('astra'), B('sol'), true);
      log('review', '@astra approved "Settings toggle"', 4);
      await step();
      hot.clear();
      caption('The commits land on your branch, fast-forward only.');
      await land(B('sol'));
      log('land', '@sol landed 4 commits on main', 1);
      await sleep(2600);
    },
    async removal() {
      gateShown = true;
      caption('@mira cleans up and deletes 562 files in its copy.');
      log('work', '@mira deleted 562 files in its copy', 0);
      hot.add('bot:mira');
      setStatus(B('mira'), 'working');
      await burst(B('mira').copy.position.clone().setY(0.04), COLOR.wait, 0.6);
      await step();
      caption('Deletions never reach your project on their own. They wait at the gate.');
      hot.add('gate');
      await removals(B('mira'), 562);
      setStatus(B('mira'), 'idle');
      log('ask', '562 removals wait for your review', 1);
      await step();
      caption('You review them by folder. One was changed 12 minutes ago, so you hold it back.');
      const card = await ask(B('mira'), REMOVAL_CARD, { lean: false });
      setStatus(B('mira'), 'idle');
      await step();
      caption('You approve logs/ and keep .freeze-probe/ waiting.');
      log('owner', 'You approved logs/ and kept .freeze-probe/ waiting', 3);
      await answer(card, REMOVAL_CARD, 0, null);
      await step();
      caption('The approved folder goes to your trash, where you can get it back. The rest keeps waiting.');
      await approveSome(22 / 562);
      log('land', '540 files moved to the trash; 22 still waiting', 1);
      await sleep(3000);
    },
  };
  async function quietSetup(plates = 2) {
    const sol = B('sol');
    sol.task = 'Settings toggle';
    setStatus(sol, 'working');
    for (let i = 0; i < plates; i++) {
      const plate = new Mesh(botGeo.plate, std(COLOR.work, { emissive: COLOR.work, emissiveIntensity: 0.25 }));
      plate.position.set(0, -0.28 + (i + 1) * 0.08, 0);
      sol.copy.add(plate);
      sol.plates.push(plate);
    }
  }

  // ---- Running a scenario -------------------------------------------------
  let current = 'run';
  let runId = 0;
  let stepping = reduced;
  let stepResolve = null;
  // In step mode the run waits here for "Next step".
  const onNextStep = [];
  function step() {
    if (!stepping) return Promise.resolve();
    return new Promise((resolve) => {
      stepResolve = () => {
        onNextStep.splice(0).forEach((done) => done());
        resolve();
      };
    });
  }
  function reset() {
    cancelAll();
    stepResolve = null;
    onNextStep.length = 0;
    for (const obj of [...dynamic]) {
      if (obj.parent) obj.parent.remove(obj);
      dynamic.delete(obj);
    }
    billboards.clear();
    for (const el of pinned.values()) el.remove();
    pinned.clear();
    tiles.length = 0;
    for (const b of bots.values()) {
      b.group.position.copy(b.home);
      b.group.lookAt(0, 0, 0);
      b.plates.forEach((p) => b.copy.remove(p));
      b.plates = [];
      b.task = null;
      b.working = 0;
      b.edgeMat.color.set(0x4c6a5d);
      setStatus(b, 'idle');
    }
    while (layers.length > BASE_LAYERS) project.remove(layers.pop());
    pickables.splice(0, pickables.length, ...pickables.filter((m) => m.parent));
    ownerHaloMat.opacity = 0;
    waitingCards = 0;
    landed = 0;
    swarm = null;
    gateShown = false;
    hot.clear();
    focusOn(null);
    clock = 14 * 60 + 2;
    clearLog();
  }
  async function play(name) {
    current = name;
    const id = ++runId;
    reset();
    for (;;) {
      try {
        await SCENARIOS[name]();
      } catch (error) {
        if (error instanceof Cancelled || id !== runId) return;
        throw error;
      }
      if (id !== runId) return;
      if (stepping) {
        caption('That is the whole run. Press Next step to watch it again.');
        await step();
        if (id !== runId) return;
      }
      reset();
    }
  }

  // ---- Controls -----------------------------------------------------------
  const scenarioButtons = [...document.querySelectorAll('.scenario')];
  scenarioButtons.forEach((button) => button.addEventListener('click', () => {
    scenarioButtons.forEach((b) => {
      b.classList.toggle('is-on', b === button);
      b.setAttribute('aria-pressed', String(b === button));
    });
    captionEl.setAttribute('aria-live', 'polite');
    setPaused(false);
    play(button.dataset.scenario);
  }));
  const toggleBtn = document.querySelector('[data-room="toggle"]');
  const speedBtn = document.querySelector('[data-room="speed"]');
  function setPaused(value) {
    paused = value;
    toggleBtn.dataset.paused = String(value);
    toggleBtn.setAttribute('aria-label', value ? 'Play the room' : 'Pause the room');
  }
  toggleBtn.addEventListener('click', () => setPaused(!paused));
  document.querySelector('[data-room="restart"]').addEventListener('click', () => { setPaused(false); play(current); });
  speedBtn.addEventListener('click', () => {
    speed = speed === 1 ? 2 : speed === 2 ? 0.5 : 1;
    speedBtn.textContent = `${speed === 0.5 ? '0.5' : speed}x`;
  });
  document.querySelector('[data-room="step"]').addEventListener('click', () => {
    const resolve = stepResolve;
    stepResolve = null;
    resolve?.();
  });
  if (stepping) {
    room.classList.add('is-stepping');
    speedBtn.hidden = true;
    toggleBtn.hidden = true;
  }

  // ---- Camera, pointer ----------------------------------------------------
  // On a wide stage the camera looks across the room, you on the right and
  // the project on the left. On a tall one it looks along that line, you at
  // the bottom and the project at the top, so the room fills a phone.
  const portrait = () => camera.aspect < 0.95;
  let yaw = MathUtils.degToRad(120);
  const pitch = () => MathUtils.degToRad(portrait() ? 50 : narrow() ? 46 : 39);
  const distance = () => MathUtils.clamp((portrait() ? 10.4 : 15.4) / (Math.tan(MathUtils.degToRad(camera.fov / 2)) * camera.aspect), 21, 44);
  let dragging = false;
  let moved = 0;
  let lastX = 0;
  let lastY = 0;
  let userYaw = 0;
  let userPitch = 0;
  // A little on the near side of the board, so the room sits above the caption bar.
  const baseLook = new Vector3();
  const look = baseLook.clone();
  function aim() {
    yaw = MathUtils.degToRad(portrait() ? 30 : 120);
    const near = portrait() ? 4.2 : 1.8;
    baseLook.set(Math.cos(yaw) * near, 0.4, Math.sin(yaw) * near);
    // The board turns with the camera, so its columns read left to right.
    board.rotation.y = Math.PI / 2 - yaw;
  }
  const pointer = new Vector2();
  const raycaster = new Raycaster();
  function targetAt(event) {
    const rect = canvas.getBoundingClientRect();
    pointer.set(((event.clientX - rect.left) / rect.width) * 2 - 1, -((event.clientY - rect.top) / rect.height) * 2 + 1);
    raycaster.setFromCamera(pointer, camera);
    const hit = raycaster.intersectObjects(pickables, false).find((h) => h.object.visible);
    return hit ? hit.object.userData.target : null;
  }
  canvas.addEventListener('pointerdown', (event) => {
    moved = 0;
    lastX = event.clientX;
    lastY = event.clientY;
    if (event.pointerType === 'mouse' || event.pointerType === 'pen') {
      dragging = true;
      stage.classList.add('is-dragging');
      canvas.setPointerCapture(event.pointerId);
    }
  });
  canvas.addEventListener('pointermove', (event) => {
    if (dragging) {
      const dx = event.clientX - lastX;
      const dy = event.clientY - lastY;
      moved += Math.abs(dx) + Math.abs(dy);
      userYaw -= dx * 0.006;
      userPitch = MathUtils.clamp(userPitch + dy * 0.004, -0.35, 0.5);
      lastX = event.clientX;
      lastY = event.clientY;
      return;
    }
    if (event.pointerType === 'mouse') stage.classList.toggle('is-pointing', Boolean(targetAt(event)));
  });
  const release = (event) => {
    if (dragging) canvas.releasePointerCapture?.(event.pointerId);
    dragging = false;
    stage.classList.remove('is-dragging');
  };
  canvas.addEventListener('pointerup', (event) => {
    const total = moved + Math.abs(event.clientX - lastX) + Math.abs(event.clientY - lastY);
    release(event);
    if (total < 6) {
      const target = targetAt(event);
      if (target) select(target);
    }
  });
  canvas.addEventListener('pointercancel', release);
  stage.classList.add('is-grab');

  // ---- Frame loop ---------------------------------------------------------
  function resize() {
    const w = stage.clientWidth;
    const h = stage.clientHeight;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.fov = w < 640 ? 46 : 36;
    camera.updateProjectionMatrix();
    aim();
  }
  new ResizeObserver(resize).observe(stage);
  resize();

  let visible = true;
  let looping = false;
  let last = performance.now();
  let elapsed = 0;
  new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting;
    if (visible) start();
  }, { threshold: 0.05 }).observe(stage);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) start(); });
  function start() {
    if (looping) return;
    looping = true;
    last = performance.now();
    requestAnimationFrame(frame);
  }
  let first = true;
  function frame(now) {
    if (!visible || document.hidden) { looping = false; return; }
    const dt = Math.min(50, now - last);
    last = now;
    advance(dt);
    if (!paused) elapsed += dt;
    const t = elapsed / 1000;
    for (const b of bots.values()) {
      const bob = reduced || b.status === 'resting' ? 0 : Math.sin(t * 2 + b.phase) * 0.035;
      b.figure.position.y = bob;
      if (!reduced) b.figure.children[3].rotation.z = t * (b.status === 'working' ? 2.4 : 0.6);
      b.copyMat.opacity = b.status === 'working' ? 0.16 + Math.sin(t * 5 + b.phase) * 0.06 : 0.08;
      if (b.status === 'waiting' && !reduced) b.haloMat.opacity = 0.45 + Math.sin(t * 6) * 0.3;
    }
    if (waitingCards && !reduced) ownerHaloMat.opacity = 0.55 + Math.sin(t * 5) * 0.3;
    if (swarm && swarm.kept < swarm.n && !reduced) swarm.points.material.opacity = 0.7 + Math.sin(t * 4) * 0.25;
    const sway = reduced || dragging ? 0 : Math.sin(t * 0.07) * 0.18;
    const y = yaw + userYaw + sway;
    const p = pitch() + userPitch;
    const k = 1 - Math.pow(0.02, dt / 1000);
    lean.lerp(leanTarget, k);
    leanZoom += (leanZoomTarget - leanZoom) * k;
    look.copy(baseLook).add(lean);
    const d = distance() * leanZoom;
    camera.position.set(look.x + Math.cos(y) * Math.cos(p) * d, look.y + Math.sin(p) * d, look.z + Math.sin(y) * Math.cos(p) * d);
    if (!reduced) {
      motes.rotation.y = t * 0.01;
      motes.position.y = Math.sin(t * 0.2) * 0.15;
    }
    camera.lookAt(look);
    for (const card of billboards) card.quaternion.copy(camera.quaternion);
    renderer.render(scene, camera);
    placeLabels();
    if (first) {
      first = false;
      stage.classList.add('is-ready');
    }
    requestAnimationFrame(frame);
  }
  start();
  clearLog();
  play('run');
}

// ---- Textures drawn on a canvas ---------------------------------------------
function boardTexture() {
  const size = 1024;
  const c = document.createElement('canvas');
  c.width = c.height = size;
  const g = c.getContext('2d');
  g.fillStyle = '#22312a';
  g.fillRect(0, 0, size, size);
  g.strokeStyle = '#3a4d44';
  g.lineWidth = 3;
  for (const x of [-0.725, 0.725]) {
    const px = size / 2 + (x / 2.62) * (size / 2);
    g.beginPath();
    g.moveTo(px, size * 0.2);
    g.lineTo(px, size * 0.8);
    g.stroke();
  }
  g.fillStyle = '#a7b2a9';
  g.font = '600 64px system-ui, sans-serif';
  g.textAlign = 'center';
  [['To do', -1.45], ['Doing', 0], ['Done', 1.45]].forEach(([text, x]) => {
    g.fillText(text, size / 2 + (x / 2.62) * (size / 2), size * 0.84);
  });
  g.font = '500 40px ui-monospace, monospace';
  g.fillStyle = '#6f8278';
  g.fillText('THE BOARD', size / 2, size * 0.14);
  const texture = new CanvasTexture(c);
  texture.colorSpace = SRGBColorSpace;
  texture.anisotropy = 4;
  return texture;
}

function cardTexture(card, chosen) {
  const w = 768;
  const h = 480;
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  const g = c.getContext('2d');
  g.fillStyle = '#ece8dc';
  g.fillRect(0, 0, w, h);
  g.fillStyle = '#82591a';
  g.fillRect(0, 0, w, 10);
  g.fillStyle = '#82591a';
  g.font = '700 30px ui-monospace, monospace';
  g.fillText(card.who.toUpperCase(), 34, 62);
  g.fillStyle = '#16211c';
  g.font = '700 46px system-ui, sans-serif';
  wrap(g, card.title, 34, 118, w - 68, 50, 2);
  g.font = '500 33px system-ui, sans-serif';
  g.fillStyle = '#2f3a33';
  card.lines.forEach((line, i) => g.fillText(line, 34, 212 + i * 44));
  let x = 34;
  card.choices.forEach((choice, i) => {
    g.font = '700 30px system-ui, sans-serif';
    const tw = g.measureText(choice).width + 40;
    const on = chosen === i;
    g.lineWidth = 4;
    g.strokeStyle = on ? '#315b4c' : '#56645b';
    if (on) {
      g.fillStyle = '#315b4c';
      g.fillRect(x, 362, tw, 74);
    }
    g.strokeRect(x, 362, tw, 74);
    g.fillStyle = on ? '#ece8dc' : '#16211c';
    g.fillText(choice, x + 20, 409);
    x += tw + 18;
  });
  const texture = new CanvasTexture(c);
  texture.colorSpace = SRGBColorSpace;
  texture.anisotropy = 8;
  return texture;
}

function wrap(g, text, x, y, max, line, lines) {
  const words = text.split(' ');
  let row = '';
  let n = 0;
  for (const word of words) {
    const next = row ? `${row} ${word}` : word;
    if (g.measureText(next).width > max && row) {
      g.fillText(row, x, y + n * line);
      row = word;
      n += 1;
      if (n >= lines) return;
    } else row = next;
  }
  g.fillText(row, x, y + n * line);
}

try {
  main();
} catch (error) {
  console.error(error);
  stage.dispatchEvent(new Event('room:fail'));
}
