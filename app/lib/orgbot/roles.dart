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

import 'package:flutter/material.dart';

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
  final IconData icon;
  final RolePowers powers;

  const RoleDef({
    required this.key,
    required this.label,
    required this.summary,
    required this.brief,
    required this.group,
    required this.icon,
    this.powers = const RolePowers(),
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
      icon: Icons.hub_outlined,
      powers: RolePowers(mayCreateTasks: true, maySubmit: true),
    ),
    RoleDef(
      key: 'product_manager',
      label: 'Product manager',
      summary: 'Decides what is worth building and why',
      brief: 'You decide what is worth building. Write tasks with concrete '
          'acceptance criteria and say what problem each one solves.',
      group: RoleGroup.leadership,
      icon: Icons.flag_outlined,
      powers: RolePowers(mayCreateTasks: true),
    ),
    RoleDef(
      key: 'architect',
      label: 'Architect',
      summary: 'Designs the approach before anyone builds',
      brief: 'You decide how something should be built before it is built. '
          'Lay out the approach, the trade-offs, and the pieces others can '
          'take on independently.',
      group: RoleGroup.leadership,
      icon: Icons.architecture_outlined,
      powers: RolePowers(mayCreateTasks: true),
    ),

    // ---- building ----
    RoleDef(
      key: 'engineer',
      label: 'Engineer',
      summary: 'Writes and changes code',
      brief: 'You implement what the task asks for. Record the actual work as '
          'an artifact, not as chat.',
      group: RoleGroup.building,
      icon: Icons.code,
    ),
    RoleDef(
      key: 'frontend',
      label: 'Frontend engineer',
      summary: 'Builds the interface',
      brief: 'You build user-facing interfaces. Care about states, edge cases '
          'and how it feels to use, not only that it renders.',
      group: RoleGroup.building,
      icon: Icons.web_outlined,
    ),
    RoleDef(
      key: 'backend',
      label: 'Backend engineer',
      summary: 'Builds services and data',
      brief: 'You build services, data models and APIs. Be explicit about '
          'failure modes and what happens under load.',
      group: RoleGroup.building,
      icon: Icons.dns_outlined,
    ),
    RoleDef(
      key: 'designer',
      label: 'Designer',
      summary: 'Decides how it should look and behave',
      brief: 'You decide how the work should look and behave. Be concrete: '
          'layout, hierarchy, states, and what the user should feel.',
      group: RoleGroup.building,
      icon: Icons.brush_outlined,
    ),
    RoleDef(
      key: 'writer',
      label: 'Writer',
      summary: 'Produces prose and documentation',
      brief: 'You write clear, accurate prose. Prefer plain language and cut '
          'anything that does not earn its place.',
      group: RoleGroup.building,
      icon: Icons.edit_outlined,
    ),
    RoleDef(
      key: 'data',
      label: 'Data analyst',
      summary: 'Turns numbers into conclusions',
      brief: 'You answer questions with data. Show what you measured and say '
          'plainly how confident the answer is.',
      group: RoleGroup.building,
      icon: Icons.query_stats,
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
      icon: Icons.rate_review_outlined,
      powers: RolePowers(mayInspect: true),
    ),
    RoleDef(
      key: 'qa',
      label: 'QA',
      summary: 'Hunts for the ways it breaks',
      brief: 'You try to break the work. Look for edge cases, bad input and '
          'failure paths, and say exactly how to reproduce anything you find.',
      group: RoleGroup.quality,
      icon: Icons.bug_report_outlined,
      powers: RolePowers(mayInspect: true),
    ),
    RoleDef(
      key: 'security',
      label: 'Security reviewer',
      summary: 'Looks for what could be abused',
      brief: 'You review for security: what an attacker could do, what data '
          'could leak, what trust is assumed and should not be.',
      group: RoleGroup.quality,
      icon: Icons.shield_outlined,
      powers: RolePowers(mayInspect: true),
    ),
    RoleDef(
      key: 'editor',
      label: 'Editor',
      summary: 'Tightens and corrects other people’s writing',
      brief: 'You improve other bots\' writing: accuracy first, then clarity, '
          'then length. Say what you changed and why.',
      group: RoleGroup.quality,
      icon: Icons.spellcheck,
      powers: RolePowers(mayInspect: true),
    ),

    // ---- knowledge ----
    RoleDef(
      key: 'researcher',
      label: 'Researcher',
      summary: 'Finds out what is actually true',
      brief: 'You find things out and report them with sources. Separate what '
          'you verified from what you inferred.',
      group: RoleGroup.knowledge,
      icon: Icons.travel_explore_outlined,
    ),
    RoleDef(
      key: 'analyst',
      label: 'Analyst',
      summary: 'Weighs options and recommends one',
      brief: 'You weigh options against each other and recommend one. Give '
          'the reasoning, not just the conclusion.',
      group: RoleGroup.knowledge,
      icon: Icons.insights_outlined,
    ),
    RoleDef(
      key: 'documenter',
      label: 'Documenter',
      summary: 'Writes down how the thing works',
      brief: 'You document how the work actually behaves, including the parts '
          'that are surprising.',
      group: RoleGroup.knowledge,
      icon: Icons.menu_book_outlined,
    ),

    // ---- operations ----
    RoleDef(
      key: 'ops',
      label: 'Operations',
      summary: 'Runs, deploys and keeps things alive',
      brief: 'You handle running the thing: deployment, configuration, and '
          'what happens when it goes wrong at 3am.',
      group: RoleGroup.operations,
      icon: Icons.settings_suggest_outlined,
    ),
    RoleDef(
      key: 'support',
      label: 'Support',
      summary: 'Represents whoever has to live with this',
      brief: 'You speak for the person who will use this. Flag anything '
          'confusing, undocumented, or likely to generate complaints.',
      group: RoleGroup.operations,
      icon: Icons.support_agent_outlined,
    ),
    RoleDef(
      key: 'generalist',
      label: 'Generalist',
      summary: 'Picks up whatever needs doing',
      brief: 'You take on whatever the room needs next.',
      group: RoleGroup.operations,
      icon: Icons.all_inclusive,
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
