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

  /// Recent bounded process and tool steps for the expandable activity rail.
  final List<ActivityStep> steps = [];
  final Set<String> _seenStepIdsThisTurn = {};

  ActivityStep? get activeStep {
    for (final step in steps.reversed) {
      if (step.running) return step;
    }
    return null;
  }

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

/// A privacy-safe summary of one model/tool operation. Arguments and results
/// are never retained here.
class ActivityStep {
  ActivityStep({required this.id, required this.label, required this.startedAt})
    : updatedAt = startedAt;

  final String id;
  final String label;
  final DateTime startedAt;
  DateTime updatedAt;
  bool running = true;
  bool? succeeded;
}

/// A readable name for a tool call: `mcp__orgbot_room__list_tasks` reads as
/// `room · list tasks`, `mcp__github__create_issue` as `github · create issue`.
String toolLabel(String name) {
  final mcp = RegExp(r'^mcp__([^_].*?)__(.+)$').firstMatch(name);
  if (mcp == null) return name;
  var server = mcp.group(1)!.replaceAll(RegExp(r'[-_]'), ' ');
  if (server == 'orgbot room') server = 'room';
  return '$server · ${mcp.group(2)!.replaceAll('_', ' ')}';
}

/// Observable room state.
class RoomStore extends ChangeNotifier {
  RoomStore(this._client, {this.notificationRoomId}) {
    _messagesSub = _client.messages.listen(_onMessage);
    _connectionSub = _client.connectionState.listen((state) {
      _connection = state;
      _notifyChanged();
    });
  }

  final RoomLink _client;
  String? notificationRoomId;
  int _notificationBatchDepth = 0;
  bool _notificationBatchDirty = false;
  Timer? _coalescedNotification;

  void _notifyChanged({bool coalesce = false}) {
    _invalidateViews();
    if (_notificationBatchDepth > 0) {
      _notificationBatchDirty = true;
      return;
    }
    if (coalesce) {
      if (_coalescedNotification != null) return;
      _coalescedNotification = Timer(const Duration(milliseconds: 16), () {
        _coalescedNotification = null;
        if (hasListeners) super.notifyListeners();
      });
      return;
    }
    _coalescedNotification?.cancel();
    _coalescedNotification = null;
    super.notifyListeners();
  }

  late final StreamSubscription<Map<String, Object?>> _messagesSub;
  late final StreamSubscription<LinkState> _connectionSub;

  void restore(Iterable<Map<String, Object?>> frames) {
    _notificationBatchDepth++;
    try {
      for (final frame in frames) {
        _onMessage(frame, deliverAttention: false);
      }
    } finally {
      _notificationBatchDepth--;
      if (_notificationBatchDepth == 0 && _notificationBatchDirty) {
        _notificationBatchDirty = false;
        _coalescedNotification?.cancel();
        _coalescedNotification = null;
        super.notifyListeners();
      }
    }
  }

  @override
  void dispose() {
    _coalescedNotification?.cancel();
    _coalescedNotification = null;
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

  /// Whether the room's state has arrived, so a summary of it means
  /// something.
  bool get hasSnapshot => _hasSnapshot;
  bool _hasSnapshot = false;
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

  /// Put a task aside until you resume it. Nobody works on it, claims it or
  /// asks you about it meanwhile, and questions waiting on it are withdrawn.
  void parkTask(String task, {String reason = ''}) =>
      _client.send({'type': 'park_task', 'task': task, 'reason': reason});

  /// Tell the room how many removals your review found waiting in a bot's
  /// copy, so its count matches what you just saw or applied.
  void reportPendingRemovals(String agent, String copy, int count) =>
      _client.send({
        'type': 'report_pending_removals',
        'agent': agent,
        'copy': copy,
        'count': count,
      });

  /// Resume a task you parked. It is open again for whoever picks it up.
  void resumeTask(String task) =>
      _client.send({'type': 'resume_task', 'task': task});

  /// Messages typed here but not yet confirmed by the engine.
  ///
  /// A bot turn can run for a minute, and the room applies a message between
  /// turns — so without this your own text sits invisible until the current
  /// turn ends, which reads as the app having swallowed it.
  final List<ChatMessage> _pending = [];
  List<ChatMessage>? _messagesView;

  Map<String, ChatMessage>? _messageIndex;
  void _invalidateMessages() {
    _messagesView = null;
    _messageIndex = null;
  }

  // Sorted views, rebuilt at most once per change instead of on every read:
  // message rows, the activity rail and the board all read them per build.
  List<Agent>? _agentsView;
  List<Task>? _tasksView;
  List<BotActivity>? _activityView;
  void _invalidateViews() {
    _agentsView = null;
    _tasksView = null;
    _activityView = null;
  }

  /// A bot by id, without sorting the roster.
  Agent? agentById(String? id) => id == null ? null : _agents[id];

  /// A transcript message by id, including ones still pending.
  ChatMessage? messageById(String id) =>
      (_messageIndex ??= {for (final m in messages) m.id: m})[id];

  /// The highest log position applied so far.
  ///
  /// On connect the server replays the room's log so nothing said before the
  /// client attached is lost. The live subscription is taken first, so a few
  /// events arrive twice; this drops the repeats rather than showing them.
  int _appliedTo = 0;

  /// A reconnect snapshot is followed by its journal, sometimes over many frames.
  bool get isReplaying => _appliedTo < _snapshotHead;

  /// The roster, in a stable order (coordinators first, then by handle).
  List<Agent> get agents => _agentsView ??= List.unmodifiable(_sortedAgents());

  List<Agent> _sortedAgents() {
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
  List<Task> get tasks => _tasksView ??= List.unmodifiable(_sortedTasks());

  List<Task> _sortedTasks() {
    final list = _tasks.values.toList();
    int rank(TaskState s) {
      if (s == TaskState.inProgress || s == TaskState.claimed) return 0;
      if (s == TaskState.blocked ||
          s == TaskState.inReview ||
          s == TaskState.approved) {
        return 1;
      }
      if (s == TaskState.ready || s == TaskState.draft) return 2;
      if (s == TaskState.parked) return 3;
      return 4;
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
      _messagesView ??= List.unmodifiable([..._messages, ..._pending]);

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
    _invalidateMessages();
    _notifyChanged();
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
    if (index >= 0) {
      _pending.removeAt(index);
      _invalidateMessages();
    }
  }

  /// Bots whose backend is currently failing, with the reason.
  ///
  /// A room where nothing happens should say why rather than looking idle.
  List<BotActivity> get failing => activity
      .where((entry) => entry.lastError != null && !entry.streaming)
      .toList();

  /// What each bot is doing, in roster order.
  List<BotActivity> get activity => _activityView ??= List.unmodifiable(
    agents.map((a) => _activity.putIfAbsent(a.id, () => BotActivity(a))),
  );

  /// Note something a bot did, for the activity panel.
  void _note(String? agentId, String action) {
    if (agentId == null) return;
    final agent = _agents[agentId];
    if (agent == null) return;
    final entry = _activity.putIfAbsent(agentId, () => BotActivity(agent));
    entry.lastAction = action;
    entry.lastActionAt = DateTime.now();
  }

  static const int _maxActivitySteps = 12;

  bool _startActivityStep(BotActivity activity, String id, String label) {
    // Some CLIs send a start notification and then repeat the completed
    // assistant block. Suppress that duplicate within this model request;
    // a fresh Thinking event permits IDs to be reused on the next request.
    if (!activity._seenStepIdsThisTurn.add(id)) return false;
    activity.steps.add(
      ActivityStep(id: id, label: toolLabel(label), startedAt: DateTime.now()),
    );
    if (activity.steps.length > _maxActivitySteps) {
      activity.steps.removeRange(0, activity.steps.length - _maxActivitySteps);
    }
    return true;
  }

  bool _finishActivityStep(
    BotActivity activity,
    String id, {
    required bool succeeded,
  }) {
    ActivityStep? step;
    for (final candidate in activity.steps.reversed) {
      if (candidate.id == id && candidate.running) {
        step = candidate;
        break;
      }
    }
    if (step == null || (!step.running && step.succeeded == succeeded)) {
      return false;
    }
    step
      ..running = false
      ..succeeded = succeeded
      ..updatedAt = DateTime.now();
    return true;
  }

  bool _finishOpenSteps(BotActivity activity) {
    var changed = false;
    for (final step in activity.steps.where((step) => step.running)) {
      step
        ..running = false
        ..succeeded = null
        ..updatedAt = DateTime.now();
      changed = true;
    }
    return changed;
  }

  void noteWorkspace(String agent, String tool) {
    _note(
      agent,
      "${tool.replaceAll('workspace_', '').replaceAll('_', ' ')} on working computer",
    );
    _notifyChanged();
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

  void _onMessage(
    Map<String, Object?> message, {
    bool deliverAttention = true,
  }) {
    _invalidateViews();
    switch (message['type'] as String?) {
      case 'snapshot':
        _applySnapshot(message);
      case 'digest':
        // Replaced wholesale: a fold over every task cannot be patched from
        // one event.
        _applyDigest((message['digest'] as Map?)?.cast<String, Object?>());
        _notifyChanged();
      case 'event':
        // The engine decided whether this is worth telling anyone about; this
        // only delivers it.
        final envelope = (message['envelope'] as Map?)?.cast<String, Object?>();
        final seq = (envelope?['seq'] as num?)?.toInt() ?? 0;
        final freshLiveEvent =
            deliverAttention &&
            seq != 0 &&
            seq > _appliedTo &&
            seq > _snapshotHead;
        _applyEvent((message['envelope'] as Map?)?.cast<String, Object?>());
        if (freshLiveEvent) {
          unawaited(
            attentions.consider(
              message,
              roomId: notificationRoomId ?? _room?.id,
            ),
          );
        }
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
          final eventType = streamEvent?['type'] as String?;
          if (eventType == 'done') {
            final activity = _activity[agentId];
            if (activity != null &&
                (activity.streaming ||
                    activity.retryStatus != null ||
                    activity.activeStep != null)) {
              activity.streaming = false;
              activity.retryStatus = null;
              _finishOpenSteps(activity);
              _notifyChanged();
            }
            return;
          }
          // Rooms can run several providers at once. A frame from one bot must
          // never erase another bot's in-flight state; each entry is cleared
          // by its own done/error/idle event.
          final retryStatus = eventType == 'retry'
              ? 'Reconnecting (${streamEvent?['attempt']}/${streamEvent?['max_retries']})'
              : null;
          final existed = _activity.containsKey(agentId);
          final activity = _activity.putIfAbsent(
            agentId,
            () => BotActivity(_agents[agentId]!),
          );
          var changed = !existed;
          if (!activity.streaming) changed = true;
          activity.streaming = true;
          if (activity.retryStatus != retryStatus) changed = true;
          activity.retryStatus = retryStatus;
          if (eventType == 'thinking') {
            activity._seenStepIdsThisTurn.clear();
          }
          var stepChanged = false;
          if (eventType == 'tool_use_start') {
            final id = streamEvent?['id'] as String?;
            final name = streamEvent?['name'] as String?;
            if (id != null && name != null) {
              stepChanged = _startActivityStep(activity, id, name);
            }
          } else if (eventType == 'tool_use_end') {
            final id = streamEvent?['id'] as String?;
            if (id != null) {
              stepChanged = _finishActivityStep(
                activity,
                id,
                succeeded: streamEvent?['succeeded'] as bool? ?? false,
              );
            }
          }
          if (changed) {
            _notifyChanged();
          } else if (stepChanged) {
            _notifyChanged(coalesce: true);
          }
        }
      case 'closed':
        _closedReason = message['reason'] as String?;
        _room?.phase = _limitReached || _closedReason == 'the room was paused'
            ? RoomPhase.paused
            : RoomPhase.completed;
        _notifyChanged();
      case 'error':
        // A lag notice: ask for a fresh snapshot to catch back up.
        lastError = message['message'] as String?;
        _notifyChanged();
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
    _hasSnapshot = true;
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
    _invalidateMessages();
    _artifacts.clear();
    _handoffs.clear();
    _notifyChanged();
  }

  void _applyPhase(String? raw) {
    final room = _room;
    if (room == null) return;
    room.phase = RoomPhase.parse(raw);
    _notifyChanged();
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
          _invalidateMessages();
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
      case 'task_amended':
        // A coordinator changed what the task asks for; reviewers judge
        // against this, so the card shows it as soon as it changes.
        final task = _tasks[event['task'] as String?];
        if (task != null) {
          final brief = event['brief'];
          if (brief is String) task.brief = brief;
          final criteria = event['acceptance_criteria'];
          if (criteria is List) {
            task.acceptanceCriteria = criteria.whereType<String>().toList();
          }
        }
      case 'agent_status_changed':
        // Leaving the faulted state (a resume, or the room retrying the bot)
        // clears the failure shown for it. Done for replayed events too: the
        // reset comes after the error in the log, so an idle bot must not keep
        // showing a failure it has been given a fresh start from.
        if (event['from'] == 'faulted' && event['to'] != 'faulted') {
          final entry = _activity[event['agent'] as String?];
          if (entry != null) {
            entry.lastError = null;
            entry.lastErrorAt = null;
            entry.errorStreak = 0;
          }
        }
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
          if (agent.status != AgentStatus.working) {
            _finishOpenSteps(entry);
          }
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
          final transition = (event['transition'] as Map?)?.cast<String, Object?>();
          final by = transition?['by'] as String?;
          if (handoff.state == 'accepted' &&
              transition?['transition'] == 'accept' &&
              by != null) {
            handoff.acceptedBy = by;
            if (handoff.kind == 'delegate') {
              _tasks[handoff.taskId]?.assignee = by;
            }
          }
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
          entry.errorStreak = (event['attempt'] as num?)?.toInt() ?? 1;
          entry.streaming = false;
          _finishOpenSteps(entry);
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
    _notifyChanged(coalesce: seq != 0 && seq <= _snapshotHead);
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
      TaskState.parked => 'parked “${task.title}”',
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
    } else if (kind == 'release' || kind == 'reopen' || kind == 'park') {
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

  bool _pauseAsked = false;

  /// Pause or resume, depending on the current phase.
  ///
  /// A pause lets the turns under way finish first, so the room keeps
  /// running for a while after it is asked to pause (see [isPausing]).
  void togglePause() {
    if (canResume) {
      _pauseAsked = false;
      _client.resume();
    } else {
      _pauseAsked = true;
      _client.pause();
      notifyListeners();
    }
  }

  /// Pause now, cutting short any turn under way. For emergencies.
  void stopNow({String reason = 'stopped from the app'}) {
    _pauseAsked = true;
    _client.stopNow(reason: reason);
    notifyListeners();
  }

  /// Asked to pause, with turns still finishing. Once the room has paused it
  /// is safe to close or update; until then work is still being saved.
  bool get isPausing => _pauseAsked && isActive;

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
    if (isPausing) return 'Pausing after current turns';
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
