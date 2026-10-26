/// Dart mirrors of the `orgbot-proto` wire types.
///
/// These parse the JSON the server sends. They are deliberately forgiving:
/// an unknown enum value becomes a labelled "unknown" rather than throwing,
/// so a client built against wire v1 keeps rendering a room from a slightly
/// newer server instead of crashing on a variant it has not learned yet.
library;

/// A point in time, milliseconds since the Unix epoch.
class Timestamp {
  const Timestamp(this.millis);

  final int millis;

  factory Timestamp.fromJson(Object? json) =>
      Timestamp((json as num?)?.toInt() ?? 0);

  DateTime get dateTime => DateTime.fromMillisecondsSinceEpoch(millis);
}

/// Where a task sits in its lifecycle. Mirrors `TaskState`.
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
  superseded,
  retired,
  archived,
  unknown;

  static TaskState parse(String? raw) {
    switch (raw) {
      case 'draft':
        return TaskState.draft;
      case 'ready':
        return TaskState.ready;
      case 'claimed':
        return TaskState.claimed;
      case 'in_progress':
        return TaskState.inProgress;
      case 'blocked':
        return TaskState.blocked;
      case 'in_review':
        return TaskState.inReview;
      case 'approved':
        return TaskState.approved;
      case 'submitted':
        return TaskState.submitted;
      case 'failed':
        return TaskState.failed;
      case 'cancelled':
        return TaskState.cancelled;
      // Permanent states. Without these three they read as `unknown`, which
      // renders as if the board had lost track of them rather than as work
      // that was deliberately replaced or set down.
      case 'superseded':
        return TaskState.superseded;
      case 'retired':
        return TaskState.retired;
      case 'archived':
        return TaskState.archived;
      default:
        return TaskState.unknown;
    }
  }

  /// A short human label for the UI.
  String get label {
    switch (this) {
      case TaskState.inProgress:
        return 'in progress';
      case TaskState.inReview:
        return 'in review';
      default:
        return name;
    }
  }

  bool get isTerminal =>
      this == TaskState.submitted ||
      this == TaskState.failed ||
      this == TaskState.cancelled;

  bool get isActive =>
      this == TaskState.claimed ||
      this == TaskState.inProgress ||
      this == TaskState.blocked ||
      this == TaskState.inReview ||
      this == TaskState.approved;
}

/// How urgent a task is.
enum Priority {
  low,
  normal,
  high,
  urgent,
  unknown;

  static Priority parse(String? raw) {
    switch (raw) {
      case 'low':
        return Priority.low;
      case 'normal':
        return Priority.normal;
      case 'high':
        return Priority.high;
      case 'urgent':
        return Priority.urgent;
      default:
        return Priority.unknown;
    }
  }
}

/// A bot's current availability. Mirrors `AgentStatus`.
enum AgentStatus {
  idle,
  working,
  waiting,
  offline,
  faulted,
  unknown;

  static AgentStatus parse(String? raw) {
    switch (raw) {
      case 'idle':
        return AgentStatus.idle;
      case 'working':
        return AgentStatus.working;
      case 'waiting':
        return AgentStatus.waiting;
      case 'offline':
        return AgentStatus.offline;
      case 'faulted':
        return AgentStatus.faulted;
      default:
        return AgentStatus.unknown;
    }
  }
}

/// Where a room sits in its lifecycle. Mirrors `RoomPhase`.
enum RoomPhase {
  assembling,
  running,
  paused,
  completed,
  aborted,
  unknown;

  static RoomPhase parse(String? raw) {
    switch (raw) {
      case 'assembling':
        return RoomPhase.assembling;
      case 'running':
        return RoomPhase.running;
      case 'paused':
        return RoomPhase.paused;
      case 'completed':
        return RoomPhase.completed;
      case 'aborted':
        return RoomPhase.aborted;
      default:
        return RoomPhase.unknown;
    }
  }

  bool get isLive => this == RoomPhase.running;
  bool get isFinished =>
      this == RoomPhase.completed || this == RoomPhase.aborted;
}

/// A bot in the room, with its live status. Mirrors `AgentView`.
class Agent {
  Agent({
    required this.id,
    required this.handle,
    required this.displayName,
    required this.roles,
    required this.status,
  });

  final String id;
  final String handle;
  final String displayName;
  final List<String> roles;
  AgentStatus status;

  factory Agent.fromJson(Map<String, Object?> json) => Agent(
    id: json['id'] as String? ?? '',
    handle: json['handle'] as String? ?? '?',
    displayName: json['display_name'] as String? ?? '',
    roles: (json['roles'] as List?)?.cast<String>() ?? const [],
    status: AgentStatus.parse(json['status'] as String?),
  );
}

/// A task on the board. Mirrors the fields of `Task` the UI needs.
class Task {
  Task({
    required this.id,
    required this.title,
    required this.brief,
    required this.state,
    required this.priority,
    required this.assignee,
    required this.blockedCount,
    required this.revision,
    required this.acceptanceCriteria,
    required this.requiredRole,
    this.dependsOn = const [],
    this.blockReasons = const [],
  });

  final String id;
  final String title;
  final String brief;
  TaskState state;
  final Priority priority;
  String? assignee;
  int blockedCount;
  int revision;
  final List<String> acceptanceCriteria;
  final String? requiredRole;

  /// Tasks that must land before this one can start. The edges of the graph.
  final List<String> dependsOn;
  List<Map<String, Object?>> blockReasons;

  factory Task.fromJson(Map<String, Object?> json) => Task(
    id: json['id'] as String? ?? '',
    title: json['title'] as String? ?? '',
    brief: json['brief'] as String? ?? '',
    state: TaskState.parse(json['state'] as String?),
    priority: Priority.parse(json['priority'] as String?),
    assignee: json['assignee'] as String?,
    blockedCount: (json['blocked_on'] as List?)?.length ?? 0,
    blockReasons: (json['blocked_on'] as List? ?? const [])
        .whereType<Map>()
        .map((r) => r.cast<String, Object?>())
        .toList(),
    revision: (json['revision'] as num?)?.toInt() ?? 0,
    acceptanceCriteria:
        (json['acceptance_criteria'] as List?)?.cast<String>() ?? const [],
    requiredRole: json['required_role'] as String?,
    dependsOn: (json['depends_on'] as List? ?? const []).cast<String>(),
  );
}

/// Who produced a chat message.
class Author {
  Author({required this.kind, this.name, this.agentId});

  /// One of `agent`, `human`, `system`.
  final String kind;

  /// The person's name, for a human author.
  final String? name;

  /// The bot's id, for an agent author.
  final String? agentId;

  factory Author.fromJson(Map<String, Object?> json) => Author(
    kind: json['kind'] as String? ?? 'system',
    name: json['name'] as String?,
    agentId: json['agent'] as String?,
  );

  bool get isSystem => kind == 'system';
  bool get isHuman => kind == 'human';
  bool get isAgent => kind == 'agent';
}

/// One message in the room transcript.
class ChatMessage {
  ChatMessage({
    required this.id,
    required this.author,
    required this.body,
    required this.intent,
    required this.taskId,
    required this.sentAt,
    this.files = const [],
    this.replyTo,
    this.audienceKind = 'everyone',
    this.audienceAgents = const [],
    this.audienceRoles = const [],
  });

  final String id;
  final Author author;
  final String body;
  final String intent;
  final String? taskId;
  final Timestamp sentAt;
  final List<ChatFile> files;
  final String? replyTo;
  final String audienceKind;
  final List<String> audienceAgents;
  final List<String> audienceRoles;

  factory ChatMessage.fromJson(Map<String, Object?> json) {
    final audience = (json['audience'] as Map?)?.cast<String, Object?>();
    return ChatMessage(
      id: json['id'] as String? ?? '',
      author: Author.fromJson(
        (json['author'] as Map?)?.cast<String, Object?>() ?? const {},
      ),
      body: json['body'] as String? ?? '',
      intent: json['intent'] as String? ?? 'say',
      taskId: json['task'] as String?,
      sentAt: Timestamp.fromJson(json['sent_at']),
      files: [
        for (final f in json['files'] as List? ?? [])
          ChatFile.fromJson((f as Map).cast<String, Object?>()),
      ],
      replyTo: json['reply_to'] as String?,
      audienceKind: audience?['kind'] as String? ?? 'everyone',
      audienceAgents: (audience?['agents'] as List? ?? const []).cast<String>(),
      audienceRoles: (audience?['roles'] as List? ?? const []).cast<String>(),
    );
  }
}

class ChatFile {
  const ChatFile({
    required this.id,
    required this.name,
    required this.mimeType,
    required this.bytes,
    this.hasText = false,
    this.images = 0,
    this.note = '',
  });
  final String id, name, mimeType, note;
  final int bytes, images;
  final bool hasText;
  factory ChatFile.fromJson(Map<String, Object?> json) => ChatFile(
    id: json['id'] as String,
    name: json['name'] as String,
    mimeType: json['mime_type'] as String? ?? 'application/octet-stream',
    bytes: (json['bytes'] as num).toInt(),
    hasText: json['has_text'] == true,
    images: (json['images'] as num?)?.toInt() ?? 0,
    note: json['note'] as String? ?? '',
  );
  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'mime_type': mimeType,
    'bytes': bytes,
    'has_text': hasText,
    'images': images,
    'note': note,
  };
}

/// A handoff between bots. Mirrors the fields the UI shows.
class Handoff {
  Handoff({
    required this.id,
    required this.taskId,
    required this.from,
    required this.kind,
    required this.state,
    required this.brief,
    this.acceptedBy,
  });

  final String id;
  final String taskId;
  final String from;
  final String kind;
  String state;
  final String brief;

  /// Who took it, once anyone has. Null while it is only offered.
  String? acceptedBy;

  /// Whether this is sitting unanswered.
  ///
  /// The distinction that matters to whoever is waiting: work nobody has
  /// picked up looks exactly like work in progress unless something says so.
  bool get awaitingAcceptance => state == 'offered';

  /// A line for whoever is waiting on it.
  String get progress => switch (state) {
    'offered' => 'nobody has picked this up yet',
    'accepted' => 'accepted and being worked on',
    'completed' => 'finished',
    'rejected' => 'declined',
    'expired' => 'nobody answered in time',
    'withdrawn' => 'pulled back',
    'failed' => 'the delegate could not finish it',
    _ => state,
  };

  factory Handoff.fromJson(Map<String, Object?> json) => Handoff(
    id: json['id'] as String? ?? '',
    taskId: json['task'] as String? ?? '',
    from: json['from'] as String? ?? '',
    kind: json['kind'] as String? ?? 'assist',
    state: json['state'] as String? ?? 'offered',
    brief: json['brief'] as String? ?? '',
    acceptedBy: json['accepted_by'] as String?,
  );
}

/// The room record from a snapshot.
class Room {
  Room({
    required this.id,
    required this.name,
    required this.goal,
    required this.phase,
    required this.turnsUsed,
    required this.maxTurns,
  });

  final String id;
  final String name;
  final String goal;
  RoomPhase phase;
  int turnsUsed;
  final int maxTurns;

  factory Room.fromJson(Map<String, Object?> json) {
    final spec = (json['spec'] as Map?)?.cast<String, Object?>() ?? const {};
    final limits =
        (spec['limits'] as Map?)?.cast<String, Object?>() ?? const {};
    return Room(
      id: json['id'] as String? ?? '',
      name: spec['name'] as String? ?? 'room',
      goal: spec['goal'] as String? ?? '',
      phase: RoomPhase.parse(json['phase'] as String?),
      turnsUsed: (json['turns_used'] as num?)?.toInt() ?? 0,
      maxTurns: (limits['max_total_turns'] as num?)?.toInt() ?? 0,
    );
  }
}

// ---------------------------------------------------------------- the digest
//
// What a person should see instead of the raw board: objectives, progress, and
// the decisions only they can make. Computed by the engine and sent whole —
// deriving it here would be a second implementation of the scheduling rules,
// and a second set of answers.

/// How much of an objective is done, counted in work that carries the outcome.
class Progress {
  const Progress({required this.done, required this.total});

  /// Carrying work delivered.
  final int done;

  /// Carrying work in total. Reviews and remediations are excluded, so
  /// inspecting something never makes the room look less finished.
  final int total;

  /// Completion as a fraction, or null when there is nothing to count.
  double? get fraction => total > 0 ? done / total : null;

  /// `7 / 10 stages`, or a plain phrase when nothing is broken down yet.
  String get label =>
      total == 0 ? 'not broken down yet' : '$done / $total stages';

  factory Progress.fromJson(Map<String, Object?> json) => Progress(
    done: (json['done'] as num?)?.toInt() ?? 0,
    total: (json['total'] as num?)?.toInt() ?? 0,
  );
}

/// One outcome, as a person should see it.
class ObjectiveCard {
  const ObjectiveCard({
    required this.task,
    required this.title,
    required this.progress,
    required this.status,
    required this.blocker,
    required this.owners,
    required this.needsOperator,
    required this.records,
  });

  /// The objective's task id.
  final String task;

  /// What it is.
  final String title;

  /// How far along.
  final Progress progress;

  /// A short state line: "in progress", "internal remediation", "needs you".
  final String status;

  /// What is holding it up right now, if anything.
  final String? blocker;

  /// The bots working on it.
  final List<String> owners;

  /// Whether a person has to do something. Shown on every card, because not
  /// knowing this is what made the old board exhausting to read.
  final bool needsOperator;

  /// Every live record beneath it, for the expanded view.
  final List<String> records;

  factory ObjectiveCard.fromJson(Map<String, Object?> json) => ObjectiveCard(
    task: json['task'] as String? ?? '',
    title: json['title'] as String? ?? '',
    progress: Progress.fromJson(
      (json['progress'] as Map?)?.cast<String, Object?>() ?? const {},
    ),
    status: json['status'] as String? ?? '',
    blocker: json['blocker'] as String?,
    owners: (json['owners'] as List? ?? const []).cast<String>(),
    needsOperator: json['needs_operator'] as bool? ?? false,
    records: (json['records'] as List? ?? const []).cast<String>(),
  );
}

/// A decision waiting on a person, with everything needed to answer it.
class PendingDecision {
  const PendingDecision({
    required this.id,
    required this.task,
    required this.title,
    required this.because,
    required this.options,
  });

  /// The question itself. An answer is addressed to this, not to a task —
  /// one answer releases every task the same question was holding.
  final String id;

  /// The task that cannot proceed.
  final String task;

  /// What that task is.
  final String title;

  /// Completes "Operator decision required because: ___".
  final String because;

  /// The answers offered, for a one-tap reply.
  final List<String> options;

  factory PendingDecision.fromJson(Map<String, Object?> json) =>
      PendingDecision(
        id: json['id'] as String? ?? '',
        task: json['task'] as String? ?? '',
        title: json['title'] as String? ?? '',
        because: json['because'] as String? ?? '',
        options: (json['options'] as List? ?? const []).cast<String>(),
      );
}

/// The headline numbers, in the order they matter.
class BoardCounts {
  const BoardCounts({
    required this.objectives,
    required this.active,
    required this.waitingInternally,
    required this.needsYou,
    required this.delivered,
    required this.rawRecords,
  });

  final int objectives;
  final int active;
  final int waitingInternally;
  final int needsYou;
  final int delivered;

  /// Every live record behind those objectives. Diagnostics only — this is the
  /// number that used to lead, and it told nobody anything.
  final int rawRecords;

  const BoardCounts.empty()
    : objectives = 0,
      active = 0,
      waitingInternally = 0,
      needsYou = 0,
      delivered = 0,
      rawRecords = 0;

  factory BoardCounts.fromJson(Map<String, Object?> json) => BoardCounts(
    objectives: (json['objectives'] as num?)?.toInt() ?? 0,
    active: (json['active'] as num?)?.toInt() ?? 0,
    waitingInternally: (json['waiting_internally'] as num?)?.toInt() ?? 0,
    needsYou: (json['needs_you'] as num?)?.toInt() ?? 0,
    delivered: (json['delivered'] as num?)?.toInt() ?? 0,
    rawRecords: (json['raw_records'] as num?)?.toInt() ?? 0,
  );
}

/// What one bot is doing.
class BotState {
  const BotState({
    required this.agent,
    required this.handle,
    required this.doing,
    required this.holding,
    required this.free,
  });

  final String agent;
  final String handle;

  /// One word: working, reviewing, waiting, idle, faulted, offline.
  final String doing;

  /// How much live work it holds.
  final int holding;

  /// Whether it could take something now.
  final bool free;

  factory BotState.fromJson(Map<String, Object?> json) => BotState(
    agent: json['agent'] as String? ?? '',
    handle: json['handle'] as String? ?? '',
    doing: json['doing'] as String? ?? 'idle',
    holding: (json['holding'] as num?)?.toInt() ?? 0,
    free: json['free'] as bool? ?? false,
  );
}

/// Where the room's effort is going.
class Utilization {
  const Utilization({
    required this.bots,
    required this.idle,
    required this.unclaimed,
    this.starvedRoles = const [],
  });

  final List<BotState> bots;
  final int idle;
  final int unclaimed;

  /// Roles that open work needs and no free bot can cover, worst first.
  final List<({String role, int tasks})> starvedRoles;

  const Utilization.empty()
    : bots = const [],
      idle = 0,
      unclaimed = 0,
      starvedRoles = const [];

  /// Open work no free bot can cover.
  int get starvedCount =>
      starvedRoles.fold(0, (sum, entry) => sum + entry.tasks);

  /// Bots free beside work they could actually take. A scheduling failure.
  bool get isWastingCapacity => idle > 0 && unclaimed > starvedCount;

  /// Work waiting on a role whose holders are all busy. A roster failure —
  /// the opposite fix from the one above, so the two must not be conflated.
  bool get isShortHanded => starvedRoles.isNotEmpty;

  factory Utilization.fromJson(Map<String, Object?> json) => Utilization(
    bots: (json['bots'] as List? ?? const [])
        .whereType<Map>()
        .map((b) => BotState.fromJson(b.cast<String, Object?>()))
        .toList(),
    idle: (json['idle'] as num?)?.toInt() ?? 0,
    unclaimed: (json['unclaimed'] as num?)?.toInt() ?? 0,
    starvedRoles: (json['starved_roles'] as List? ?? const [])
        .whereType<List>()
        .map(
          (pair) => (
            role: pair.isNotEmpty ? pair[0] as String : '',
            tasks: pair.length > 1 ? (pair[1] as num).toInt() : 0,
          ),
        )
        .toList(),
  );
}

/// Signals that the room is coordinating more than it is working.
class RoomHealth {
  const RoomHealth({
    required this.inflationRatio,
    required this.supportingRecords,
    required this.coordinationExpansion,
    required this.averageReviewCycles,
    required this.standInRecords,
    required this.criticalPathLength,
  });

  /// Live records per objective.
  final double inflationRatio;

  /// Records that exist only to support other records.
  final int supportingRecords;

  /// Whether the record count is outrunning the outcomes.
  final bool coordinationExpansion;

  /// Mean review rounds per reviewed task. Above two means work routinely
  /// goes round more than once.
  final double averageReviewCycles;

  /// Records done by a bot standing in for the role asked for. A high share
  /// means the roster does not match the work.
  final int standInRecords;

  /// How long the chain deciding completion is.
  final int criticalPathLength;

  const RoomHealth.healthy()
    : inflationRatio = 0,
      supportingRecords = 0,
      coordinationExpansion = false,
      averageReviewCycles = 0,
      standInRecords = 0,
      criticalPathLength = 0;

  factory RoomHealth.fromJson(Map<String, Object?> json) => RoomHealth(
    inflationRatio: (json['inflation_ratio'] as num?)?.toDouble() ?? 0,
    supportingRecords: (json['supporting_records'] as num?)?.toInt() ?? 0,
    coordinationExpansion: json['coordination_expansion'] as bool? ?? false,
    averageReviewCycles:
        (json['average_review_cycles'] as num?)?.toDouble() ?? 0,
    standInRecords: (json['stand_in_records'] as num?)?.toInt() ?? 0,
    criticalPathLength: (json['critical_path_length'] as num?)?.toInt() ?? 0,
  );
}

/// Everything a person needs, and nothing they do not.
class RoomDigest {
  const RoomDigest({
    required this.objectives,
    required this.counts,
    required this.needsYou,
    required this.criticalPath,
    required this.health,
    required this.utilization,
    this.spend = const [],
    this.agentSpend = const [],
    this.totalSpend = const RoomSpend(),
  });

  /// The outcomes, most urgent first.
  final List<ObjectiveCard> objectives;

  /// The headline numbers.
  final BoardCounts counts;

  /// The operator's inbox. Usually empty.
  final List<PendingDecision> needsYou;

  /// The chain currently deciding when the room finishes.
  final List<String> criticalPath;

  /// Whether the room is coordinating more than it is working.
  final RoomHealth health;

  /// Where the room's effort is going.
  final Utilization utilization;

  /// What the room has drawn, by the account it drew it from.
  ///
  /// Grouped by account, not by bot, because that is the unit an allowance is
  /// granted in: a roster mostly on one subscription draws on one limit, and
  /// per-bot numbers hide that entirely.
  final List<AccountSpend> spend;
  final List<AgentSpend> agentSpend;
  final RoomSpend totalSpend;

  /// The state before a room has said anything.
  const RoomDigest.empty()
    : objectives = const [],
      counts = const BoardCounts.empty(),
      needsYou = const [],
      criticalPath = const [],
      health = const RoomHealth.healthy(),
      utilization = const Utilization.empty(),
      spend = const [],
      agentSpend = const [],
      totalSpend = const RoomSpend();

  /// Whether there is anything to show yet.
  bool get isEmpty => objectives.isEmpty && needsYou.isEmpty;

  factory RoomDigest.fromJson(Map<String, Object?> json) => RoomDigest(
    objectives: (json['objectives'] as List? ?? const [])
        .whereType<Map>()
        .map((o) => ObjectiveCard.fromJson(o.cast<String, Object?>()))
        .toList(),
    counts: BoardCounts.fromJson(
      (json['counts'] as Map?)?.cast<String, Object?>() ?? const {},
    ),
    needsYou: (json['needs_you'] as List? ?? const [])
        .whereType<Map>()
        .map((d) => PendingDecision.fromJson(d.cast<String, Object?>()))
        .toList(),
    criticalPath: (json['critical_path'] as List? ?? const []).cast<String>(),
    health: RoomHealth.fromJson(
      (json['health'] as Map?)?.cast<String, Object?>() ?? const {},
    ),
    utilization: Utilization.fromJson(
      (json['utilization'] as Map?)?.cast<String, Object?>() ?? const {},
    ),
    spend: (json['spend'] as List? ?? const [])
        .whereType<Map>()
        .map((s) => AccountSpend.fromJson(s.cast<String, Object?>()))
        .toList(),
    agentSpend: (json['agent_spend'] as List? ?? const [])
        .whereType<Map>()
        .map((s) => AgentSpend.fromJson(s.cast<String, Object?>()))
        .toList(),
    totalSpend: RoomSpend.fromJson(
      (json['total_spend'] as Map?)?.cast<String, Object?>() ?? const {},
    ),
  );
}

/// What one account has been asked for.
class AccountSpend {
  const AccountSpend({
    required this.backend,
    required this.turns,
    required this.tokens,
    required this.perTurn,
    this.inputTokens = 0,
    this.outputTokens = 0,
    this.cacheReadTokens = 0,
    this.cacheWriteTokens = 0,
    this.usageKnown = false,
    this.usageComplete = false,
    this.actualApiCostMicrousd,
    this.apiEquivalentCostMicrousd,
  });

  /// The account, as the scheduler groups it.
  final String backend;

  /// Turns taken against it.
  final int turns;

  /// Tokens across those turns. Check usageKnown and usageComplete before
  /// treating zero as a real count.
  final int tokens;

  /// Average tokens per turn.
  final int perTurn;
  final int inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens;
  final bool usageKnown, usageComplete;
  final int? actualApiCostMicrousd, apiEquivalentCostMicrousd;

  /// Whether the backend reported anything at all.
  bool get isMetered => usageKnown;

  factory AccountSpend.fromJson(Map<String, Object?> json) => AccountSpend(
    backend: json['backend'] as String? ?? '',
    turns: (json['turns'] as num?)?.toInt() ?? 0,
    tokens: (json['tokens'] as num?)?.toInt() ?? 0,
    perTurn: (json['per_turn'] as num?)?.toInt() ?? 0,
    inputTokens: (json['input_tokens'] as num?)?.toInt() ?? 0,
    outputTokens: (json['output_tokens'] as num?)?.toInt() ?? 0,
    cacheReadTokens: (json['cache_read_tokens'] as num?)?.toInt() ?? 0,
    cacheWriteTokens: (json['cache_write_tokens'] as num?)?.toInt() ?? 0,
    usageKnown: json['usage_known'] == true,
    usageComplete: json.containsKey('usage_complete')
        ? json['usage_complete'] == true
        : json['usage_known'] == true,
    actualApiCostMicrousd: (json['actual_api_cost_microusd'] as num?)?.toInt(),
    apiEquivalentCostMicrousd: (json['api_equivalent_cost_microusd'] as num?)
        ?.toInt(),
  );
}

class AgentSpend {
  const AgentSpend({
    required this.handle,
    required this.turns,
    required this.inputTokens,
    required this.outputTokens,
    required this.cacheReadTokens,
    required this.cacheWriteTokens,
    required this.usageKnown,
    required this.usageComplete,
    this.actualApiCostMicrousd,
    this.apiEquivalentCostMicrousd,
  });

  final String handle;
  final int turns, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens;
  final bool usageKnown, usageComplete;
  final int? actualApiCostMicrousd, apiEquivalentCostMicrousd;

  factory AgentSpend.fromJson(Map<String, Object?> json) => AgentSpend(
    handle: json['handle'] as String? ?? '',
    turns: (json['turns'] as num?)?.toInt() ?? 0,
    inputTokens: (json['input_tokens'] as num?)?.toInt() ?? 0,
    outputTokens: (json['output_tokens'] as num?)?.toInt() ?? 0,
    cacheReadTokens: (json['cache_read_tokens'] as num?)?.toInt() ?? 0,
    cacheWriteTokens: (json['cache_write_tokens'] as num?)?.toInt() ?? 0,
    usageKnown: json['usage_known'] == true,
    usageComplete: json.containsKey('usage_complete')
        ? json['usage_complete'] == true
        : json['usage_known'] == true,
    actualApiCostMicrousd: (json['actual_api_cost_microusd'] as num?)?.toInt(),
    apiEquivalentCostMicrousd: (json['api_equivalent_cost_microusd'] as num?)
        ?.toInt(),
  );
}

class RoomSpend {
  const RoomSpend({
    this.turns = 0,
    this.inputTokens = 0,
    this.outputTokens = 0,
    this.cacheReadTokens = 0,
    this.cacheWriteTokens = 0,
    this.usageKnown = false,
    this.usageComplete = false,
    this.actualApiCostMicrousd,
    this.apiEquivalentCostMicrousd,
  });

  final int turns, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens;
  final bool usageKnown, usageComplete;
  final int? actualApiCostMicrousd, apiEquivalentCostMicrousd;

  factory RoomSpend.fromJson(Map<String, Object?> json) => RoomSpend(
    turns: (json['turns'] as num?)?.toInt() ?? 0,
    inputTokens: (json['input_tokens'] as num?)?.toInt() ?? 0,
    outputTokens: (json['output_tokens'] as num?)?.toInt() ?? 0,
    cacheReadTokens: (json['cache_read_tokens'] as num?)?.toInt() ?? 0,
    cacheWriteTokens: (json['cache_write_tokens'] as num?)?.toInt() ?? 0,
    usageKnown: json['usage_known'] == true,
    usageComplete: json.containsKey('usage_complete')
        ? json['usage_complete'] == true
        : json['usage_known'] == true,
    actualApiCostMicrousd: (json['actual_api_cost_microusd'] as num?)?.toInt(),
    apiEquivalentCostMicrousd: (json['api_equivalent_cost_microusd'] as num?)
        ?.toInt(),
  );
}
