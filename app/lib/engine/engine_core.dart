// ignore_for_file: prefer_interpolation_to_compose_strings

// The on-device coordination engine: the Dart mirror of gptbots-engine's
// task and handoff state machines, plus the room that applies commands.
// Everything here is pure Dart with no I/O, so it runs and is testable on
// Android exactly as on the host. The desktop/server engines stay the
// reference; this port keeps the same vocabulary and rules.
library;

import 'dart:math';

/// A short random id, in the wire shape (task_..., agent_..., ...).
String genId(String prefix) {
  final rnd = Random.secure();
  final bytes = List.generate(8, (_) => rnd.nextInt(256));
  return prefix +
      '_' +
      bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
}

/// The legal life of a task. Mirrors gptbots-proto::task::TaskState.
enum TaskState {
  draft,
  ready,
  claimed,
  inProgress,
  blocked,
  inReview,
  approved,
  submitted,
  failed,
  cancelled,
}

extension TaskStateX on TaskState {
  bool get isTerminal =>
      this == TaskState.submitted || this == TaskState.failed || this == TaskState.cancelled;

  bool get isLive => !isTerminal && this != TaskState.draft;

  /// Every legal transition, keyed by the engine operation name.
  /// Illegal moves are absent, so the model can never tempt the engine.
  TaskState? after(String op) {
    switch (this) {
      case TaskState.draft:
        if (op == 'open') return TaskState.ready;
        if (op == 'cancel') return TaskState.cancelled;
      case TaskState.ready:
        if (op == 'claim') return TaskState.claimed;
        if (op == 'cancel') return TaskState.cancelled;
      case TaskState.claimed:
        if (op == 'start') return TaskState.inProgress;
        if (op == 'release') return TaskState.ready;
      case TaskState.inProgress:
        if (op == 'submit_for_review') return TaskState.inReview;
        if (op == 'block') return TaskState.blocked;
      case TaskState.blocked:
        if (op == 'unblock') return TaskState.inProgress;
      case TaskState.inReview:
        if (op == 'approve') return TaskState.approved;
        if (op == 'request_changes') return TaskState.inProgress;
        if (op == 'fail') return TaskState.failed;
      case TaskState.approved:
        if (op == 'submit') return TaskState.submitted;
      default:
        break;
    }
    return null;
  }
}

class EngineError implements Exception {
  final String message;
  EngineError(this.message);
  @override
  String toString() => message;
}

/// One task on the board.
class Task {
  final String id;
  final String title;
  final String brief;
  TaskState state;
  final String? requiredRole;
  String? assignee;
  int priority; // higher first
  int revision;
  int maxRevisions;
  final List<String> artifacts;
  String? parent;

  Task({
    String? id,
    required this.title,
    required this.brief,
    this.state = TaskState.draft,
    this.requiredRole,
    this.assignee,
    this.priority = 1,
    this.revision = 0,
    this.maxRevisions = 3,
    this.parent,
  })  : id = id ?? genId('task'),
        artifacts = [];
}

/// A handoff: one bot asking another to take over work. Mirrors handoff.rs.
enum HandoffState { offered, accepted, completed, rejected, withdrawn, expired }

class Handoff {
  final String id;
  final String task;
  final String from;
  final String? toAgent;
  final String? toRole;
  final String? reason;
  HandoffState state;
  final String createdAt;

  Handoff({
    String? id,
    required this.task,
    required this.from,
    this.toAgent,
    this.toRole,
    this.reason,
    this.state = HandoffState.offered,
  })  : id = id ?? genId('handoff'),
        createdAt = DateTime.now().toIso8601String();
}

/// A bot: who it is and what it may do.
class Agent {
  final String id;
  final String handle;
  final List<String> roles;
  final bool mayCreateTasks;
  final bool mayInspect;
  final bool maySubmit;

  Agent({
    String? id,
    required this.handle,
    this.roles = const [],
    this.mayCreateTasks = false,
    this.mayInspect = false,
    this.maySubmit = false,
  }) : id = id ?? genId('agent');

  bool canTake(String? required) => required == null || roles.contains(required);
}

enum RoomPhase { idle, running, paused, completed, failed }

/// One chat message on the log.
class Message {
  final String id;
  final String author; // handle, or 'human'
  final String body;
  final DateTime at;
  Message(this.author, this.body, {String? id})
      : id = id ?? genId('msg'),
        at = DateTime.now();
}

/// The outcome of a tool call against the engine.
class ToolOutcome {
  final String content;
  final bool isError;
  const ToolOutcome.ok(this.content) : isError = false;
  const ToolOutcome.error(this.content) : isError = true;
}

/// The room: agents, board, handoffs, transcript. One process, no I/O.
class Engine {
  final String name;
  final String goal;
  RoomPhase phase = RoomPhase.idle;
  int turnsUsed = 0;
  final Map<String, Agent> agents = {};
  final Map<String, Task> tasks = {};
  final Map<String, Handoff> handoffs = {};
  final List<Message> messages = [];
  final List<String> log = []; // append-only event log, human readable

  Engine({required this.name, required this.goal});

  // ---- roster ----
  Agent requireAgent(String id) => agents[id] ?? (throw EngineError('unknown agent ' + id));
  Task requireTask(String id) => tasks[id] ?? (throw EngineError('unknown task ' + id));

  /// Seed the board directly (no actor, no autonomy check) - the phone
  /// equivalent of a room file's [[tasks]].
  String seedTask(String title, String brief, {String? requiredRole}) {
    final t = Task(
      title: title,
      brief: brief,
      requiredRole: requiredRole,
    );
    tasks[t.id] = t;
    _open(t);
    _event('task', t.id + ' seeded: ' + title);
    return t.id;
  }

  void seat(Agent a) {
    if (agents.containsKey(a.id)) throw EngineError('agent ' + a.handle + ' already seated');
    agents[a.id] = a;
    _event('joined', a.handle);
  }

  void start() {
    if (agents.isEmpty) throw EngineError('a room needs at least one bot');
    phase = RoomPhase.running;
    _event('room', 'running');
  }

  void pause() => phase = RoomPhase.paused;
  void resume() => phase = RoomPhase.running;

  // ---- public tool surface ----
  ToolOutcome call(String name, Map<String, Object?> args, String actorId) {
    try {
      if (phase != RoomPhase.running) {
        return ToolOutcome.error('room is ' + phase.name + '; only a running room accepts moves');
      }
      switch (name) {
        case 'room_status':
          return _roomStatus();
        case 'create_task':
          return ToolOutcome.ok('created ' + _createTask(args, actorId).id);
        case 'list_tasks':
          return _listTasks(args, actorId);
        case 'claim_task':
          return _claim(args, actorId);
        case 'start_task':
          return _start(args, actorId);
        case 'add_artifact':
          return _addArtifact(args, actorId);
        case 'submit_for_review':
          return _submitForReview(args, actorId);
        case 'release_task':
          return _release(args, actorId);
        case 'record_review':
          return _review(args, actorId);
        case 'submit_task':
          return _submit(args, actorId);
        case 'offer_handoff':
          return _offerHandoff(args, actorId);
        case 'accept_handoff':
          return _acceptHandoff(args, actorId);
        case 'reject_handoff':
          return _rejectHandoff(args, actorId);
        case 'complete_handoff':
          return _completeHandoff(args, actorId);
        case 'post_message':
          return _postMessage(args, actorId);
        default:
          return ToolOutcome.error('no tool called ' + name);
      }
    } on EngineError catch (e) {
      return ToolOutcome.error(e.message);
    }
  }

  // ---- internals ----
  String _str(Map<String, Object?> args, String key, [String? dflt]) {
    final v = args[key];
    if (v is String && v.trim().isNotEmpty) return v;
    if (dflt != null) return dflt;
    throw EngineError('field is required and must be a non-empty string: ' + key);
  }

  void _event(String kind, String detail) {
    log.add(DateTime.now().toIso8601String() + ' ' + kind + ': ' + detail);
  }

  ToolOutcome _roomStatus() {
    final b = StringBuffer('Room: ' + name + ' - ' + goal + '\n');
    b.writeln('Phase: ' + phase.name + ', ' + turnsUsed.toString() + ' turns used');
    b.writeln('Bots:');
    for (final a in agents.values) {
      b.writeln('  @' + a.handle + ' [' + a.roles.join('/') + ']');
    }
    b.writeln('Board:');
    for (final t in tasks.values) {
      b.writeln('  ' + t.state.name + ' ' + t.id + ' - ' + t.title +
          (t.assignee != null ? ' (@' + (agents[t.assignee]?.handle ?? '?') + ')' : ''));
    }
    return ToolOutcome.ok(b.toString());
  }

  Task _createTask(Map<String, Object?> args, String actorId) {
    final by = requireAgent(actorId);
    if (!by.mayCreateTasks) throw EngineError('@' + by.handle + ' may not create tasks');
    final t = Task(
      title: _str(args, 'title'),
      brief: _str(args, 'brief'),
      requiredRole: args['required_role'] as String?,
      priority: (args['priority'] as num?)?.toInt() ?? 1,
      parent: args['parent'] as String?,
    );
    tasks[t.id] = t;
    _open(t);
    _event('task', t.id + ' created and opened');
    return t;
  }

  void _open(Task t) {
    final next = t.state.after('open');
    if (next == null) throw EngineError('task ' + t.id + ' cannot open from ' + t.state.name);
    t.state = next;
  }

  ToolOutcome _listTasks(Map<String, Object?> args, String actorId) {
    final filter = args['filter'] as String? ?? 'all';
    final by = requireAgent(actorId);
    final list = tasks.values.where((t) {
      switch (filter) {
        case 'mine':
          return t.assignee == actorId;
        case 'claimable':
          return t.state == TaskState.ready && by.canTake(t.requiredRole);
        case 'in_review':
          return t.state == TaskState.inReview;
        default:
          return true;
      }
    });
    if (list.isEmpty) return ToolOutcome.ok('no tasks');
    final b = StringBuffer();
    for (final t in list) {
      b.writeln(t.state.name + ' ' + t.id + ' - ' + t.title);
    }
    return ToolOutcome.ok(b.toString().trim());
  }

  ToolOutcome _claim(Map<String, Object?> args, String actorId) {
    final by = requireAgent(actorId);
    final t = requireTask(_str(args, 'task'));
    if (!by.canTake(t.requiredRole)) {
      throw EngineError('task ' + t.id + ' needs role ' +
          (t.requiredRole ?? '-') + ', @' + by.handle + ' is ' + by.roles.join('/'));
    }
    final next = t.state.after('claim');
    if (next == null) throw EngineError('task ' + t.id + ' is not claimable (' + t.state.name + ')');
    t.state = next;
    t.assignee = actorId;
    _event('task', t.id + ' claimed by @' + by.handle);
    return ToolOutcome.ok(t.id + ' is now claimed.');
  }

  void _requireAssignee(Task t, String actorId, String op) {
    if (t.assignee != actorId) throw EngineError('only the assignee may ' + op + ' ' + t.id);
  }

  ToolOutcome _start(Map<String, Object?> args, String actorId) {
    final t = requireTask(_str(args, 'task'));
    _requireAssignee(t, actorId, 'start');
    t.state = t.state.after('start')!;
    _event('task', t.id + ' started');
    return ToolOutcome.ok(t.id + ' is in progress.');
  }

  ToolOutcome _addArtifact(Map<String, Object?> args, String actorId) {
    final t = requireTask(_str(args, 'task'));
    _requireAssignee(t, actorId, 'add artifacts to');
    final title = _str(args, 'title', 'artifact');
    _str(args, 'body'); // validated; the artifact is stored by id
    final id = genId('artifact');
    t.artifacts.add(id);
    _event('artifact', t.id + ' got ' + id + ' (' + title + ')');
    return ToolOutcome.ok('added artifact ' + id + ' to ' + t.id);
  }

  ToolOutcome _submitForReview(Map<String, Object?> args, String actorId) {
    final t = requireTask(_str(args, 'task'));
    _requireAssignee(t, actorId, 'submit');
    if (t.artifacts.isEmpty) {
      return ToolOutcome.error('add an artifact first - there is nothing to review');
    }
    t.state = t.state.after('submit_for_review')!;
    _event('review', t.id + ' awaiting review');
    return ToolOutcome.ok(t.id + ' is awaiting review.');
  }

  ToolOutcome _release(Map<String, Object?> args, String actorId) {
    final t = requireTask(_str(args, 'task'));
    _requireAssignee(t, actorId, 'release');
    t.state = t.state.after('release')!;
    t.assignee = null;
    _event('task', t.id + ' released');
    return ToolOutcome.ok(t.id + ' is back on the board.');
  }

  ToolOutcome _review(Map<String, Object?> args, String actorId) {
    final inspector = requireAgent(actorId);
    if (!inspector.mayInspect) throw EngineError('@' + inspector.handle + ' may not inspect work');
    final t = requireTask(_str(args, 'task'));
    if (t.assignee == actorId) throw EngineError('a bot cannot inspect its own work');
    if (t.state != TaskState.inReview) {
      throw EngineError(t.id + ' is not awaiting review (' + t.state.name + ')');
    }
    final summary = _str(args, 'summary');
    final findings = (args['findings'] as List?) ?? [];
    final major = findings
        .where((f) => f is Map && f['severity'] == 'major')
        .isNotEmpty;
    final critical = findings
        .where((f) => f is Map && f['severity'] == 'critical')
        .isNotEmpty;
    if (critical) {
      t.state = t.state.after('fail')!;
      _event('review', t.id + ' failed by @' + inspector.handle);
      return ToolOutcome.ok(t.id + ' failed inspection: ' + summary);
    }
    if (major) {
      if (t.revision + 1 >= t.maxRevisions) {
        t.state = TaskState.failed;
        return ToolOutcome.ok(t.id + ' exceeded its revision budget and failed.');
      }
      t.state = t.state.after('request_changes')!;
      t.revision += 1;
      _event('review', t.id + ' sent back (rev ' + t.revision.toString() + ')');
      return ToolOutcome.ok(t.id + ' sent back for rework: ' + summary);
    }
    t.state = t.state.after('approve')!;
    _event('review', t.id + ' approved by @' + inspector.handle);
    return ToolOutcome.ok(t.id + ' approved.');
  }

  ToolOutcome _submit(Map<String, Object?> args, String actorId) {
    final by = requireAgent(actorId);
    if (!by.maySubmit) throw EngineError('@' + by.handle + ' may not submit final work');
    final t = requireTask(_str(args, 'task'));
    final next = t.state.after('submit');
    if (next == null) throw EngineError(t.id + ' is not approved (' + t.state.name + ')');
    t.state = next;
    _event('task', t.id + ' submitted');
    return ToolOutcome.ok(t.id + ' submitted.');
  }

  ToolOutcome _offerHandoff(Map<String, Object?> args, String actorId) {
    final t = requireTask(_str(args, 'task'));
    _requireAssignee(t, actorId, 'hand off');
    final h = Handoff(
      task: t.id,
      from: actorId,
      toAgent: args['to_agent'] as String?,
      toRole: args['to_role'] as String?,
      reason: args['reason'] as String?,
    );
    if (h.toAgent == null && h.toRole == null) {
      throw EngineError('a handoff needs a recipient (to_agent or to_role)');
    }
    handoffs[h.id] = h;
    _event('handoff', h.id + ' offered for ' + t.id);
    return ToolOutcome.ok(h.id + ' offered for ' + t.id);
  }

  Handoff _requireHandoff(String id, String actorId, String op) {
    final h = handoffs[id] ?? (throw EngineError('unknown handoff ' + id));
    if (op == 'accept' || op == 'reject') {
      final me = requireAgent(actorId);
      final addressed = h.toAgent == actorId ||
          (h.toRole != null && me.roles.contains(h.toRole));
      if (!addressed) throw EngineError('handoff ' + id + ' was not offered to you');
    } else {
      if (h.from != actorId) throw EngineError('only the sender may ' + op + ' handoff ' + id);
    }
    return h;
  }

  ToolOutcome _acceptHandoff(Map<String, Object?> args, String actorId) {
    final h = _requireHandoff(_str(args, 'handoff'), actorId, 'accept');
    if (h.state != HandoffState.offered) {
      throw EngineError('handoff ' + h.id + ' is ' + h.state.name + ', not offered');
    }
    h.state = HandoffState.accepted;
    final t = tasks[h.task]!;
    t.assignee = actorId;
    _event('handoff', h.id + ' accepted');
    return ToolOutcome.ok(h.id + ' accepted; ' + t.id + ' is yours.');
  }

  ToolOutcome _rejectHandoff(Map<String, Object?> args, String actorId) {
    final h = _requireHandoff(_str(args, 'handoff'), actorId, 'reject');
    if (h.state != HandoffState.offered) {
      throw EngineError('handoff ' + h.id + ' is ' + h.state.name + ', not offered');
    }
    h.state = HandoffState.rejected;
    return ToolOutcome.ok(h.id + ' rejected.');
  }

  ToolOutcome _completeHandoff(Map<String, Object?> args, String actorId) {
    final h = _requireHandoff(_str(args, 'handoff'), actorId, 'complete');
    if (h.state != HandoffState.accepted) {
      throw EngineError('handoff ' + h.id + ' is ' + h.state.name + '; only an accepted handoff completes');
    }
    h.state = HandoffState.completed;
    return ToolOutcome.ok(h.id + ' completed.');
  }

  ToolOutcome _postMessage(Map<String, Object?> args, String actorId) {
    final by = requireAgent(actorId);
    final body = _str(args, 'body');
    messages.add(Message('@' + by.handle, body));
    _event('chat', '@' + by.handle + ': ' + body);
    return ToolOutcome.ok('posted');
  }
}

/// Whether the room is done: every task terminal.
bool allTasksTerminal(Engine engine) {
  if (engine.tasks.isEmpty) return false;
  return engine.tasks.values.every((t) => t.state.isTerminal);
}
