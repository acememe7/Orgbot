/// The client-side model of a room.
///
/// A [RoomStore] subscribes to a [RoomClient], applies the snapshot and the
/// event tail, and exposes the result as a [ChangeNotifier] the UI listens to.
/// The event-folding here mirrors the server's own projections — the same log
/// that rebuilds a room on the server rebuilds this view — so a reconnecting
/// client converges on identical state.
library;

import 'dart:async';

import '../orgbot/attention.dart';

import 'package:flutter/foundation.dart';

import '../net/room_client.dart';
import '../protocol/models.dart';

/// What one bot is doing right now, for the activity panel.
///
/// The chat shows what bots *say*; this shows what they are *doing*, which is
/// most of the work and was previously invisible.
class BotActivity {
  BotActivity(this.agent);

  final Agent agent;

  /// The task it currently holds, if any.
  String? taskId;
  String? taskTitle;

  /// The most recent thing it did, in plain words.
  String? lastAction;

  /// When that happened, for an "x ago" label.
  DateTime? lastActionAt;

  /// True while a model turn is streaming, so the UI can show it thinking.
  bool streaming = false;
  String? retryStatus;

  /// The most recent failure from this bot's backend, if it is failing.
  ///
  /// Without this a bot whose provider cannot start looks identical to one
  /// with nothing to do — which is how a room where every turn failed to
  /// spawn appeared as ten bots sitting quietly at "ready".
  String? lastError;

  /// The original event time, retained when reopening saved history.
  DateTime? lastErrorAt;

  /// How many turns in a row have failed.
  int errorStreak = 0;

  /// A running count of what it has got done.
  int tasksSubmitted = 0;
  int reviewsGiven = 0;
  int handoffsSent = 0;
}

/// Observable room state.
class RoomStore extends ChangeNotifier {
  RoomStore(this._client) {
    _messagesSub = _client.messages.listen(_onMessage);
    _connectionSub = _client.connectionState.listen((state) {
      _connection = state;
      notifyListeners();
    });
  }

  final RoomLink _client;
  late final StreamSubscription<Map<String, Object?>> _messagesSub;
  late final StreamSubscription<LinkState> _connectionSub;

  void restore(Iterable<Map<String, Object?>> frames) {
    for (final frame in frames) {
      _onMessage(frame);
    }
  }

  @override
  void dispose() {
    _messagesSub.cancel();
    _connectionSub.cancel();
    super.dispose();
  }

  String? lastError;
  String? _closedReason;

  LinkState _connection = LinkState.disconnected;
  LinkState get connection => _connection;

  Room? _room;
  Room? get room => _room;

  final Map<String, Agent> _agents = {};
  int _snapshotHead = 0;
  final Map<String, int> _snapshotErrors = {};
  final Map<String, Task> _tasks = {};
  final Map<String, Handoff> _handoffs = {};
  final List<ChatMessage> _messages = [];
  final Map<String, Map<String, Object?>> _artifacts = {};
  List<Map<String, Object?>> get artifacts => _artifacts.values.toList();
  final Map<String, BotActivity> _activity = {};

  /// The board as a person should see it, computed by the engine.
  ///
  /// Never derived here: it folds every task through the same rules the
  /// scheduler claims work by, and a second implementation in the UI would be
  /// a second set of answers.
  RoomDigest _digest = const RoomDigest.empty();

  /// Objectives, progress, and the decisions only you can make.
  RoomDigest get digest => _digest;

  /// Tells you when the room needs you. Shared across rooms so a single
  /// permission prompt and a single de-duplication set cover all of them.
  static Attentions attentions = Attentions();

  /// Answer a question the room asked. Releases every task it was holding.
  void answerDecision(String decision, String chosen, {String note = ''}) =>
      _client.answerDecision(decision, chosen, note: note);

  /// Messages typed here but not yet confirmed by the engine.
  ///
  /// A bot turn can run for a minute, and the room applies a message between
  /// turns — so without this your own text sits invisible until the current
  /// turn ends, which reads as the app having swallowed it.
  final List<ChatMessage> _pending = [];

  /// The highest log position applied so far.
  ///
  /// On connect the server replays the room's log so nothing said before the
  /// client attached is lost. The live subscription is taken first, so a few
  /// events arrive twice; this drops the repeats rather than showing them.
  int _appliedTo = 0;

  /// A reconnect snapshot is followed by its journal, sometimes over many frames.
  bool get isReplaying => _appliedTo < _snapshotHead;

  /// The roster, in a stable order (coordinators first, then by handle).
  List<Agent> get agents {
    final list = _agents.values.toList();
    list.sort((a, b) {
      final ra = a.roles.contains('coordinator') ? 0 : 1;
      final rb = b.roles.contains('coordinator') ? 0 : 1;
      if (ra != rb) return ra - rb;
      return a.handle.compareTo(b.handle);
    });
    return list;
  }

  /// The board, most-recently-active first is not tracked; ordered by state
  /// group (active, waiting, open, done) then title so it reads like a board.
  List<Task> get tasks {
    final list = _tasks.values.toList();
    int rank(TaskState s) {
      if (s == TaskState.inProgress || s == TaskState.claimed) return 0;
      if (s == TaskState.blocked ||
          s == TaskState.inReview ||
          s == TaskState.approved) {
        return 1;
      }
      if (s == TaskState.ready || s == TaskState.draft) return 2;
      return 3;
    }

    list.sort((a, b) {
      final r = rank(a.state) - rank(b.state);
      return r != 0 ? r : a.title.compareTo(b.title);
    });
    return list;
  }

  /// The transcript, oldest first, with anything you have said but the room
  /// has not confirmed yet shown at the end.
  List<ChatMessage> get messages =>
      List.unmodifiable([..._messages, ..._pending]);

  /// Whether `message` is still awaiting confirmation from the room.
  bool isPending(ChatMessage message) =>
      _pending.any((candidate) => identical(candidate, message));

  /// Show something you just said, before the room has echoed it back.
  void addPending(
    String author,
    String body, {
    List<ChatFile> files = const [],
  }) {
    final text = body.trim();
    if (text.isEmpty) return;
    _pending.add(
      ChatMessage(
        id: 'pending_${DateTime.now().microsecondsSinceEpoch}',
        author: Author(kind: 'human', name: author),
        body: text,
        intent: 'ask',
        taskId: null,
        sentAt: Timestamp(DateTime.now().millisecondsSinceEpoch),
        files: files,
      ),
    );
    notifyListeners();
  }

  /// Drop a pending message once the room has confirmed the same text.
  void _confirmPending(ChatMessage arrived) {
    if (!arrived.author.isHuman) return;
    final index = _pending.indexWhere(
      (candidate) =>
          candidate.body == arrived.body &&
          candidate.author.name == arrived.author.name &&
          listEquals(
            candidate.files.map((f) => f.id).toList(),
            arrived.files.map((f) => f.id).toList(),
          ),
    );
    if (index >= 0) _pending.removeAt(index);
  }

  /// Bots whose backend is currently failing, with the reason.
  ///
  /// A room where nothing happens should say why rather than looking idle.
  List<BotActivity> get failing => activity
      .where((entry) => entry.lastError != null && !entry.streaming)
      .toList();

  /// What each bot is doing, in roster order.
  List<BotActivity> get activity => agents
      .map((a) => _activity.putIfAbsent(a.id, () => BotActivity(a)))
      .toList();

  /// Note something a bot did, for the activity panel.
  void _note(String? agentId, String action) {
    if (agentId == null) return;
    final agent = _agents[agentId];
    if (agent == null) return;
    final entry = _activity.putIfAbsent(agentId, () => BotActivity(agent));
    entry.lastAction = action;
    entry.lastActionAt = DateTime.now();
  }

  void noteWorkspace(String agent, String tool) {
    _note(
      agent,
      "${tool.replaceAll('workspace_', '').replaceAll('_', ' ')} on working computer",
    );
    notifyListeners();
  }

  /// Pending handoffs, for a status strip.
  /// Every handoff, so a view can say whether a delegation was picked up.
  List<Handoff> get handoffs => _handoffs.values.toList();

  List<Handoff> get openHandoffs => _handoffs.values
      .where((h) => h.state == 'offered' || h.state == 'accepted')
      .toList();

  /// Look up a bot's handle by id, for rendering authors and assignees.
  String handleFor(String? agentId) {
    if (agentId == null) return 'unassigned';
    return _agents[agentId]?.handle ?? _short(agentId);
  }

  static String _short(String id) {
    final us = id.indexOf('_');
    if (us < 0 || id.length < us + 7) return id;
    return id.substring(0, us + 7);
  }

  // ---- inbound message handling --------------------------------------------

  void _onMessage(Map<String, Object?> message) {
    switch (message['type'] as String?) {
      case 'snapshot':
        _applySnapshot(message);
      case 'digest':
        // Replaced wholesale: a fold over every task cannot be patched from
        // one event.
        _applyDigest((message['digest'] as Map?)?.cast<String, Object?>());
        notifyListeners();
      case 'event':
        // The engine decided whether this is worth telling anyone about; this
        // only delivers it.
        unawaited(attentions.consider(message));
        _applyEvent((message['envelope'] as Map?)?.cast<String, Object?>());
      case 'phase':
        _applyPhase(message['phase'] as String?);
      case 'stream':
        // Deltas do not go in the transcript — the durable event that follows
        // carries the final text — but they are the clearest signal that a bot
        // is actively thinking, which the activity panel shows.
        final agentId = message['agent'] as String?;
        if (agentId != null && _agents.containsKey(agentId)) {
          // A delayed stream completion can arrive after the durable idle event.
          // It must clear the indicator, never start another "thinking" state.
          final streamEvent = message['event'] as Map?;
          if (streamEvent?['type'] == 'done') {
            final activity = _activity[agentId];
            if (activity != null &&
                (activity.streaming || activity.retryStatus != null)) {
              activity.streaming = false;
              activity.retryStatus = null;
              notifyListeners();
            }
            return;
          }
          // Exactly one bot holds a turn at a time, so a delta from this one
          // means every other is no longer composing. Without clearing them
          // the flags accumulate and the room claims nine bots are thinking.
          var changed = false;
          final retryStatus = streamEvent?['type'] == 'retry'
              ? 'Reconnecting (${streamEvent?['attempt']}/${streamEvent?['max_retries']})'
              : null;
          for (final entry in _activity.entries) {
            final streaming = entry.key == agentId;
            if (entry.value.streaming != streaming) {
              entry.value.streaming = streaming;
              changed = true;
            }
            final retry = entry.key == agentId ? retryStatus : null;
            if (entry.value.retryStatus != retry) {
              entry.value.retryStatus = retry;
              changed = true;
            }
          }
          final existed = _activity.containsKey(agentId);
          final activity = _activity.putIfAbsent(
            agentId,
            () => BotActivity(_agents[agentId]!),
          );
          if (!existed) {
            changed = true;
          }
          activity.streaming = true;
          activity.retryStatus = retryStatus;
          if (changed) {
            notifyListeners();
          }
        }
      case 'closed':
        _closedReason = message['reason'] as String?;
        _room?.phase = _limitReached || _closedReason == 'the room was paused'
            ? RoomPhase.paused
            : RoomPhase.completed;
        notifyListeners();
      case 'error':
        // A lag notice: ask for a fresh snapshot to catch back up.
        lastError = message['message'] as String?;
        notifyListeners();
    }
  }

  /// Take a new digest. Does not notify: a snapshot applies this partway
  /// through and must not rebuild the UI over half-applied state.
  void _applyDigest(Map<String, Object?>? digest) {
    if (digest == null) return;
    _digest = RoomDigest.fromJson(digest);
  }

  void _applySnapshot(Map<String, Object?> snapshot) {
    _applyDigest((snapshot['digest'] as Map?)?.cast<String, Object?>());
    final roomJson = (snapshot['room'] as Map?)?.cast<String, Object?>();
    if (roomJson != null) _room = Room.fromJson(roomJson);

    _closedReason = null;
    _appliedTo = 0;
    _snapshotHead = (snapshot['head'] as num?)?.toInt() ?? 0;
    _snapshotErrors.clear();
    _activity.clear();
    _agents.clear();
    for (final entry in (snapshot['roster'] as List? ?? const [])) {
      final agent = Agent.fromJson((entry as Map).cast<String, Object?>());
      _agents[agent.id] = agent;
      if (entry['error_streak'] is num) {
        _snapshotErrors[agent.id] = (entry['error_streak'] as num).toInt();
      }
    }

    _tasks.clear();
    for (final entry in (snapshot['tasks'] as List? ?? const [])) {
      final task = Task.fromJson((entry as Map).cast<String, Object?>());
      _tasks[task.id] = task;
    }
    // A fresh snapshot replaces everything; the transcript is rebuilt from the
    // event tail that follows.
    _messages.clear();
    _artifacts.clear();
    _handoffs.clear();
    notifyListeners();
  }

  void _applyPhase(String? raw) {
    final room = _room;
    if (room == null) return;
    room.phase = RoomPhase.parse(raw);
    notifyListeners();
  }

  void _applyEvent(Map<String, Object?>? envelope) {
    if (envelope == null) return;
    final seq = (envelope['seq'] as num?)?.toInt() ?? 0;
    // A replayed event the client has already seen.
    if (seq != 0 && seq <= _appliedTo) return;
    if (seq > _appliedTo) _appliedTo = seq;
    final event = (envelope['event'] as Map?)?.cast<String, Object?>();
    if (event == null) return;

    switch (event['event'] as String?) {
      case 'message_posted':
        final json = (event['message'] as Map?)?.cast<String, Object?>();
        if (json != null) {
          final message = ChatMessage.fromJson(json);
          _confirmPending(message);
          _messages.add(message);
          final activity = _activity[message.author.agentId];
          if (activity != null) {
            activity.lastError = null;
            activity.lastErrorAt = null;
            activity.errorStreak = 0;
          }
        }
      case 'artifact_added':
        final artifact = (event['artifact'] as Map?)?.cast<String, Object?>();
        if (artifact != null) _artifacts[artifact['id'] as String] = artifact;
      case 'task_created':
        final json = (event['task'] as Map?)?.cast<String, Object?>();
        if (json != null) {
          final task = Task.fromJson(json);
          _tasks[task.id] = task;
        }
      case 'task_transitioned':
        _applyTaskTransition(event);
      case 'agent_status_changed':
        // The snapshot is newer than its replayed journal. A formerly faulted
        // bot may now have a repaired model and be ready to run.
        if (seq <= _snapshotHead) break;
        final id = event['agent'] as String?;
        final agent = id == null ? null : _agents[id];
        if (agent != null) {
          agent.status = AgentStatus.parse(event['to'] as String?);
          final entry = _activity.putIfAbsent(id!, () => BotActivity(agent));
          // A turn has finished once the bot is no longer working.
          if (agent.status != AgentStatus.working) entry.streaming = false;
        }
      case 'handoff_offered':
        final json = (event['handoff'] as Map?)?.cast<String, Object?>();
        if (json != null) {
          final handoff = Handoff.fromJson(json);
          _handoffs[handoff.id] = handoff;
          _note(handoff.from, 'asked for help: ${handoff.brief}');
          _activity[handoff.from]?.handoffsSent += 1;
        }
      case 'handoff_transitioned':
        final id = event['handoff'] as String?;
        final handoff = id == null ? null : _handoffs[id];
        if (handoff != null) {
          handoff.state = event['to'] as String? ?? handoff.state;
        }
      case 'review_recorded':
        final review = (event['review'] as Map?)?.cast<String, Object?>();
        final inspector = review?['inspector'] as String?;
        final verdict = review?['verdict'] as String?;
        if (inspector != null) {
          _note(inspector, 'reviewed work — $verdict');
          _activity[inspector]?.reviewsGiven += 1;
        }
      case 'agent_errored':
        final agentId = event['agent'] as String?;
        if (seq <= _snapshotHead && _snapshotErrors[agentId] == 0) break;
        final agent = agentId == null ? null : _agents[agentId];
        if (agent != null && agentId != null) {
          final entry = _activity.putIfAbsent(
            agentId,
            () => BotActivity(agent),
          );
          entry.lastError = event['error'] as String? ?? 'the turn failed';
          final at = (envelope['at'] as num?)?.toInt();
          entry.lastErrorAt = at == null
              ? null
              : DateTime.fromMillisecondsSinceEpoch(at);
          for (final other in _activity.values) {
            other.streaming = false;
          }
          entry.errorStreak = (event['attempt'] as num?)?.toInt() ?? 1;
          entry.streaming = false;
          _note(agentId, 'turn failed: ${entry.lastError}');
          entry.lastActionAt = entry.lastErrorAt;
        }
      case 'room_closed':
        _closedReason = event['reason'] as String?;
        _room?.phase = RoomPhase.completed;
      case 'agent_joined':
        final spec = (event['spec'] as Map?)?.cast<String, Object?>();
        if (spec != null) {
          final agent = Agent(
            id: spec['id'] as String? ?? '',
            handle: spec['handle'] as String? ?? '?',
            displayName: spec['display_name'] as String? ?? '',
            roles: (spec['roles'] as List?)?.cast<String>() ?? const [],
            status: AgentStatus.idle,
          );
          _agents.putIfAbsent(agent.id, () => agent);
        }
    }

    // Keep the room's turn counter roughly current from any event burst.
    notifyListeners();
  }

  void _applyTaskTransition(Map<String, Object?> event) {
    final id = event['task'] as String?;
    final task = id == null ? null : _tasks[id];
    if (task == null) return;
    task.state = TaskState.parse(event['to'] as String?);

    // Describe the change the way a person would say it.
    final actor = event['actor'] as String?;
    final phrase = switch (task.state) {
      TaskState.claimed => 'took “${task.title}”',
      TaskState.inProgress => 'is working on “${task.title}”',
      TaskState.blocked => 'is blocked on “${task.title}”',
      TaskState.inReview => 'sent “${task.title}” for review',
      TaskState.approved => 'approved “${task.title}”',
      TaskState.submitted => 'delivered “${task.title}”',
      TaskState.failed => 'gave up on “${task.title}”',
      _ => null,
    };
    if (phrase != null) _note(actor, phrase);
    if (actor != null && task.state == TaskState.submitted) {
      _activity[actor]?.tasksSubmitted += 1;
    }
    if (actor != null) {
      final entry = _activity[actor];
      if (entry != null) {
        // Show which task a bot is on, and clear it once it lets go.
        if (task.state.isActive) {
          entry.taskId = task.id;
          entry.taskTitle = task.title;
        } else if (entry.taskId == task.id) {
          entry.taskId = null;
          entry.taskTitle = null;
        }
      }
    }
    // Ownership and blockers are derived from the transition kind.
    final transition = (event['transition'] as Map?)?.cast<String, Object?>();
    final kind = transition?['transition'] as String?;
    if (kind == 'block') {
      final reason = transition?['reason'];
      if (reason is Map &&
          !task.blockReasons.any((r) => mapEquals(r, reason))) {
        task.blockReasons = [
          ...task.blockReasons,
          reason.cast<String, Object?>(),
        ];
      }
      task.blockedCount = task.blockReasons.length;
    } else if (task.state != TaskState.blocked) {
      task.blockReasons = [];
    }
    if (kind == 'claim') {
      task.assignee = transition?['by'] as String?;
    } else if (kind == 'release' || kind == 'reopen') {
      task.assignee = null;
      task.blockedCount = 0;
    } else if (kind == 'unblock') {
      task.blockedCount = 0;
    } else if (kind == 'request_changes') {
      task.revision += 1;
    }
  }

  // ---- outbound actions ----------------------------------------------------

  void answer(String task, String body) {
    if (body.trim().isNotEmpty) _client.answerBot(task, body.trim());
  }

  /// Post a human message.
  void send(String author, String body) {
    if (body.trim().isEmpty) return;
    _client.sendMessage(author, body.trim());
  }

  /// Pause or resume, depending on the current phase.
  void togglePause() {
    if (canResume) {
      _client.resume();
    } else {
      _client.pause();
    }
  }

  /// Whether the room is paused.
  bool get isActive => _room?.phase == RoomPhase.running;
  bool get isPaused => _room?.phase == RoomPhase.paused;
  bool get isCompleted => _room?.phase == RoomPhase.completed;
  bool get canResume => isPaused || isCompleted;
  bool get _limitReached =>
      _closedReason == 'the room ran out of time' ||
      _closedReason == 'the room spent its turn budget';

  /// Older checkpoints called exhausted budgets completed; don't claim success.
  String get runStateLabel {
    if (isActive) return 'Active';
    if (isCompleted && _limitReached) {
      return _closedReason == 'the room ran out of time'
          ? 'Time limit reached'
          : 'Turn limit reached';
    }
    return isCompleted
        ? 'Completed'
        : isPaused
        ? 'Paused'
        : 'Saved';
  }
}
