// What a bot does in a room, and what it is allowed to do.
//
// A role used to be one of three strings picked while creating a room, which
// was wrong twice over: a bot's job is a property of the bot, not of one room
// it happens to be in, and three options cannot describe an organisation.
//
// A role carries two things: permissions the engine enforces, and a line of
// standing instruction that shapes the bot's prompt. Separation of duties is
// the reason permissions are not a free-for-all — an author must never be able
// to approve its own work, and a planner must not sign off on its own plan.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

/// What a bot is permitted to do on the board.
class RolePowers {
  /// May add tasks to the board.
  final bool mayCreateTasks;

  /// May inspect other bots' work and record a verdict.
  final bool mayInspect;

  /// May deliver approved work. This is the final, irreversible step.
  final bool maySubmit;

  const RolePowers({
    this.mayCreateTasks = false,
    this.mayInspect = false,
    this.maySubmit = false,
  });

  RolePowers copyWith({bool? mayCreateTasks, bool? mayInspect, bool? maySubmit}) =>
      RolePowers(
        mayCreateTasks: mayCreateTasks ?? this.mayCreateTasks,
        mayInspect: mayInspect ?? this.mayInspect,
        maySubmit: maySubmit ?? this.maySubmit,
      );

  Map<String, Object?> toJson() => {
        'may_create_tasks': mayCreateTasks,
        'may_inspect': mayInspect,
        'may_submit': maySubmit,
      };

  factory RolePowers.fromJson(Map<String, Object?> json) => RolePowers(
        mayCreateTasks: json['may_create_tasks'] as bool? ?? false,
        mayInspect: json['may_inspect'] as bool? ?? false,
        maySubmit: json['may_submit'] as bool? ?? false,
      );
}

/// Which part of an organisation a role belongs to, used to group the picker.
enum RoleGroup {
  leadership('Leadership'),
  building('Building'),
  quality('Quality'),
  knowledge('Knowledge'),
  operations('Operations');

  final String label;
  const RoleGroup(this.label);
}

/// A job a bot can hold.
class RoleDef {
  /// Stable key, stored with the bot and written into the room file.
  final String key;

  /// What the picker shows.
  final String label;

  /// One line explaining the job, shown under the name.
  final String summary;

  /// Appended to the bot's system prompt so it knows its remit.
  final String brief;

  final RoleGroup group;

  /// Names the icon the UI should draw. Kept as a string so this file stays
  /// plain Dart — a data model that needs the widget toolkit cannot be used
  /// by tooling, tests, or anything off the UI thread.
  final String icon;

  final RolePowers powers;

  /// What holders of this role can do. Work may ask for a capability rather
  /// than a job title, which is often what it actually needs.
  final List<String> capabilities;

  /// Roles that may take this one's work when every holder is busy.
  ///
  /// Named explicitly rather than inferred from shared capabilities: two roles
  /// doing similar things does not make them interchangeable, and guessing
  /// that they are puts work with the wrong bot. An exact holder always wins,
  /// so this only fires when the room would otherwise sit still.
  final List<String> fallbacks;

  const RoleDef({
    required this.key,
    required this.label,
    required this.summary,
    required this.brief,
    required this.group,
    required this.icon,
    this.powers = const RolePowers(),
    this.capabilities = const [],
    this.fallbacks = const [],
  });
}

/// Every role a bot can hold.
class Roles {
  static const List<RoleDef> all = [
    // ---- leadership: plans and delivers, never inspects its own plan ----
    RoleDef(
      key: 'coordinator',
      label: 'Coordinator',
      summary: 'Breaks the goal into tasks and delivers finished work',
      brief: 'You break the room goal into clear tasks others can work in '
          'parallel, keep the room moving, and deliver work once it passes '
          'inspection. You do not inspect work yourself.',
      group: RoleGroup.leadership,
      icon: 'hub_outlined',
      powers: RolePowers(mayCreateTasks: true, maySubmit: true),
      capabilities: ['plan', 'deliver'],
      fallbacks: ['product_manager'],
    ),
    RoleDef(
      key: 'product_manager',
      label: 'Product manager',
      summary: 'Decides what is worth building and why',
      brief: 'You decide what is worth building. Write tasks with concrete '
          'acceptance criteria and say what problem each one solves.',
      group: RoleGroup.leadership,
      icon: 'flag_outlined',
      powers: RolePowers(mayCreateTasks: true),
      capabilities: ['plan'],
      fallbacks: ['coordinator'],
    ),
    RoleDef(
      key: 'architect',
      label: 'Architect',
      summary: 'Designs the approach before anyone builds',
      brief: 'You decide how something should be built before it is built. '
          'Lay out the approach, the trade-offs, and the pieces others can '
          'take on independently.',
      group: RoleGroup.leadership,
      icon: 'architecture_outlined',
      powers: RolePowers(mayCreateTasks: true),
      capabilities: ['design', 'review', 'write'],
      fallbacks: ['engineer'],
    ),

    // ---- building ----
    RoleDef(
      key: 'engineer',
      label: 'Engineer',
      summary: 'Writes and changes code',
      brief: 'You implement what the task asks for. Record the actual work as '
          'an artifact, not as chat.',
      group: RoleGroup.building,
      icon: 'code',
      capabilities: ['build', 'fix', 'test'],
      fallbacks: ['backend', 'frontend', 'architect'],
    ),
    RoleDef(
      key: 'frontend',
      label: 'Frontend engineer',
      summary: 'Builds the interface',
      brief: 'You build user-facing interfaces. Care about states, edge cases '
          'and how it feels to use, not only that it renders.',
      group: RoleGroup.building,
      icon: 'web_outlined',
      capabilities: ['build', 'ui'],
      fallbacks: ['engineer', 'designer'],
    ),
    RoleDef(
      key: 'backend',
      label: 'Backend engineer',
      summary: 'Builds services and data',
      brief: 'You build services, data models and APIs. Be explicit about '
          'failure modes and what happens under load.',
      group: RoleGroup.building,
      icon: 'dns_outlined',
      capabilities: ['build', 'data', 'infra'],
      fallbacks: ['engineer'],
    ),
    RoleDef(
      key: 'designer',
      label: 'Designer',
      summary: 'Decides how it should look and behave',
      brief: 'You decide how the work should look and behave. Be concrete: '
          'layout, hierarchy, states, and what the user should feel.',
      group: RoleGroup.building,
      icon: 'brush_outlined',
      capabilities: ['ui', 'design'],
      fallbacks: ['frontend'],
    ),
    RoleDef(
      key: 'writer',
      label: 'Writer',
      summary: 'Produces prose and documentation',
      brief: 'You write clear, accurate prose. Prefer plain language and cut '
          'anything that does not earn its place.',
      group: RoleGroup.building,
      icon: 'edit_outlined',
      capabilities: ['write', 'document'],
      fallbacks: ['editor', 'documenter'],
    ),
    RoleDef(
      key: 'data',
      label: 'Data analyst',
      summary: 'Turns numbers into conclusions',
      brief: 'You answer questions with data. Show what you measured and say '
          'plainly how confident the answer is.',
      group: RoleGroup.building,
      icon: 'query_stats',
      capabilities: ['data', 'analyse'],
      fallbacks: ['analyst', 'backend'],
    ),

    // ---- quality: may inspect, never submits its own verdict onward ----
    RoleDef(
      key: 'reviewer',
      label: 'Reviewer',
      summary: 'Inspects work against its acceptance criteria',
      brief: 'You inspect finished work against its acceptance criteria, one '
          'by one. Report what you actually found; the verdict follows from '
          'your findings, so do not soften them.',
      group: RoleGroup.quality,
      icon: 'rate_review_outlined',
      powers: RolePowers(mayInspect: true),
      capabilities: ['review', 'audit'],
      fallbacks: ['architect', 'qa'],
    ),
    RoleDef(
      key: 'qa',
      label: 'QA',
      summary: 'Hunts for the ways it breaks',
      brief: 'You try to break the work. Look for edge cases, bad input and '
          'failure paths, and say exactly how to reproduce anything you find.',
      group: RoleGroup.quality,
      icon: 'bug_report_outlined',
      powers: RolePowers(mayInspect: true),
      capabilities: ['test', 'review'],
      fallbacks: ['engineer', 'reviewer'],
    ),
    RoleDef(
      key: 'security',
      label: 'Security reviewer',
      summary: 'Looks for what could be abused',
      brief: 'You review for security: what an attacker could do, what data '
          'could leak, what trust is assumed and should not be.',
      group: RoleGroup.quality,
      icon: 'shield_outlined',
      powers: RolePowers(mayInspect: true),
      capabilities: ['audit', 'review'],
    ),
    RoleDef(
      key: 'editor',
      label: 'Editor',
      summary: 'Tightens and corrects other people’s writing',
      brief: 'You improve other bots\' writing: accuracy first, then clarity, '
          'then length. Say what you changed and why.',
      group: RoleGroup.quality,
      icon: 'spellcheck',
      powers: RolePowers(mayInspect: true),
      capabilities: ['write', 'document'],
      fallbacks: ['writer', 'documenter'],
    ),

    // ---- knowledge ----
    RoleDef(
      key: 'researcher',
      label: 'Researcher',
      summary: 'Finds out what is actually true',
      brief: 'You find things out and report them with sources. Separate what '
          'you verified from what you inferred.',
      group: RoleGroup.knowledge,
      icon: 'travel_explore_outlined',
      capabilities: ['investigate', 'gather'],
      fallbacks: ['analyst', 'generalist', 'product_manager'],
    ),
    RoleDef(
      key: 'analyst',
      label: 'Analyst',
      summary: 'Weighs options and recommends one',
      brief: 'You weigh options against each other and recommend one. Give '
          'the reasoning, not just the conclusion.',
      group: RoleGroup.knowledge,
      icon: 'insights_outlined',
      capabilities: ['analyse', 'investigate'],
      fallbacks: ['researcher', 'data'],
    ),
    RoleDef(
      key: 'documenter',
      label: 'Documenter',
      summary: 'Writes down how the thing works',
      brief: 'You document how the work actually behaves, including the parts '
          'that are surprising.',
      group: RoleGroup.knowledge,
      icon: 'menu_book_outlined',
      capabilities: ['document', 'write'],
      fallbacks: ['writer', 'editor'],
    ),

    // ---- operations ----
    RoleDef(
      key: 'ops',
      label: 'Operations',
      summary: 'Runs, deploys and keeps things alive',
      brief: 'You handle running the thing: deployment, configuration, and '
          'what happens when it goes wrong at 3am.',
      group: RoleGroup.operations,
      icon: 'settings_suggest_outlined',
      capabilities: ['infra', 'deploy'],
      fallbacks: ['backend', 'engineer'],
    ),
    RoleDef(
      key: 'support',
      label: 'Support',
      summary: 'Represents whoever has to live with this',
      brief: 'You speak for the person who will use this. Flag anything '
          'confusing, undocumented, or likely to generate complaints.',
      group: RoleGroup.operations,
      icon: 'support_agent_outlined',
      capabilities: ['investigate'],
      fallbacks: ['generalist'],
    ),
    RoleDef(
      key: 'generalist',
      label: 'Generalist',
      summary: 'Picks up whatever needs doing',
      brief: 'You take on whatever the room needs next.',
      group: RoleGroup.operations,
      icon: 'all_inclusive',
      capabilities: ['build', 'write', 'investigate'],
    ),
  ];

  /// Look a role up, falling back to the generalist so an unknown key saved by
  /// an older build still loads.
  static RoleDef byKey(String key) => all.firstWhere(
        (r) => r.key == key,
        orElse: () => all.firstWhere((r) => r.key == 'generalist'),
      );

  /// The roles in a group, for a grouped picker.
  static List<RoleDef> inGroup(RoleGroup group) =>
      all.where((r) => r.group == group).toList();

  /// The default for a new bot.
  static const String defaultKey = 'engineer';
}
