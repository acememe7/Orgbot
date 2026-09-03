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

  factory Timestamp.fromJson(Object? json) => Timestamp((json as num?)?.toInt() ?? 0);

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
      this == TaskState.submitted || this == TaskState.failed || this == TaskState.cancelled;

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
  bool get isFinished => this == RoomPhase.completed || this == RoomPhase.aborted;
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

  factory Task.fromJson(Map<String, Object?> json) => Task(
        id: json['id'] as String? ?? '',
        title: json['title'] as String? ?? '',
        brief: json['brief'] as String? ?? '',
        state: TaskState.parse(json['state'] as String?),
        priority: Priority.parse(json['priority'] as String?),
        assignee: json['assignee'] as String?,
        blockedCount: (json['blocked_on'] as List?)?.length ?? 0,
        revision: (json['revision'] as num?)?.toInt() ?? 0,
        acceptanceCriteria:
            (json['acceptance_criteria'] as List?)?.cast<String>() ?? const [],
        requiredRole: json['required_role'] as String?,
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
  });

  final String id;
  final Author author;
  final String body;
  final String intent;
  final String? taskId;
  final Timestamp sentAt;

  factory ChatMessage.fromJson(Map<String, Object?> json) => ChatMessage(
        id: json['id'] as String? ?? '',
        author: Author.fromJson((json['author'] as Map?)?.cast<String, Object?>() ?? const {}),
        body: json['body'] as String? ?? '',
        intent: json['intent'] as String? ?? 'say',
        taskId: json['task'] as String?,
        sentAt: Timestamp.fromJson(json['sent_at']),
      );
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
  });

  final String id;
  final String taskId;
  final String from;
  final String kind;
  String state;
  final String brief;

  factory Handoff.fromJson(Map<String, Object?> json) => Handoff(
        id: json['id'] as String? ?? '',
        taskId: json['task'] as String? ?? '',
        from: json['from'] as String? ?? '',
        kind: json['kind'] as String? ?? 'assist',
        state: json['state'] as String? ?? 'offered',
        brief: json['brief'] as String? ?? '',
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
    final limits = (spec['limits'] as Map?)?.cast<String, Object?>() ?? const {};
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
