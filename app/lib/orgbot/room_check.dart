// Whether a room can actually finish the work you give it.
//
// A room with one engineer in it looks fine and then stalls: the bot claims a
// task, does it, sends it for review — and nothing can review it, because
// inspecting your own work is forbidden and nobody else is there. The board
// reads "0 of 1 delivered, 1 in flight" forever.
//
// That is a composition problem, not a bug, and the place to catch it is when
// the room is being built rather than ten minutes into a run.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'models.dart';
import 'roles.dart';

/// How badly a problem affects the room.
enum CheckSeverity {
  /// The room cannot complete its work as configured.
  blocking,

  /// The room will run, but probably not as well as you expect.
  advisory,
}

/// Something worth telling the person before they start the room.
class RoomIssue {
  final CheckSeverity severity;

  /// What is wrong, in one line.
  final String summary;

  /// What to do about it.
  final String fix;

  const RoomIssue(this.severity, this.summary, this.fix);
}

/// Check whether a roster can carry a piece of work end to end.
///
/// The pipeline is: someone does the work, someone else inspects it, and
/// someone delivers it. A gap anywhere means tasks pile up at that step.
List<RoomIssue> checkRoom(RoomDef room, List<BotDef> bots) {
  final issues = <RoomIssue>[];

  if (bots.isEmpty) {
    return const [
      RoomIssue(CheckSeverity.blocking, 'This room has no bots in it.',
          'Add at least one bot that can do the work.'),
    ];
  }

  final powers = bots.map((bot) => bot.effectivePowers).toList();
  final inspectors = bots.where((b) => b.effectivePowers.mayInspect).toList();
  final submitters = powers.where((p) => p.maySubmit).length;
  final planners = powers.where((p) => p.mayCreateTasks).length;
  // Someone who actually produces something. Defined by the role's group
  // rather than by permissions: a coordinator has no inspect right either, but
  // it plans and delivers — it does not build, so counting it as a worker
  // would hide a room that has nobody doing the work.
  final workers = bots.where((b) {
    final group = Roles.byKey(b.role).group;
    return group == RoleGroup.building ||
        group == RoleGroup.knowledge ||
        group == RoleGroup.operations;
  }).length;

  if (inspectors.isEmpty) {
    issues.add(const RoomIssue(
      CheckSeverity.blocking,
      'Nobody can inspect work.',
      'Add a bot with a quality role — Reviewer, QA, Security or Editor. '
          'A bot may never approve its own work, so finished tasks will sit in '
          'review forever without one.',
    ));
  }

  if (submitters == 0) {
    issues.add(const RoomIssue(
      CheckSeverity.blocking,
      'Nobody can deliver approved work.',
      'Add a Coordinator, or switch on "Delivers" for one of your bots. '
          'Without it, work is approved and then never handed over.',
    ));
  }

  if (workers == 0) {
    issues.add(const RoomIssue(
      CheckSeverity.blocking,
      'Every bot here inspects; nobody does the work.',
      'Add a building role such as Engineer, Writer or Designer.',
    ));
  }

  // A single bot cannot both produce and inspect, whatever its permissions.
  if (bots.length == 1) {
    issues.add(const RoomIssue(
      CheckSeverity.blocking,
      'A room of one bot cannot finish anything.',
      'Work has to be inspected by a bot other than its author, so a room '
          'needs at least two — normally three: someone to plan, someone to '
          'build, and someone to check.',
    ));
  }

  if (planners == 0 && room.tasks.isEmpty) {
    issues.add(const RoomIssue(
      CheckSeverity.blocking,
      'There is no work and nobody who can create any.',
      'Give the room a task, or add a Coordinator, Product manager or '
          'Architect who can break the goal down.',
    ));
  }

  // Advisory: the room runs, but is lopsided.
  if (inspectors.length == 1 && bots.length > 4) {
    issues.add(RoomIssue(
      CheckSeverity.advisory,
      'One inspector for ' + bots.length.toString() + ' bots.',
      'Everything funnels through it, so it will become the bottleneck.',
    ));
  }
  if (room.goal.trim().isEmpty) {
    issues.add(const RoomIssue(
      CheckSeverity.advisory,
      'The room has no goal.',
      'Every bot is shown the goal each turn; without one they have little to '
          'steer by.',
    ));
  }

  return issues;
}

/// Whether anything found would stop the room finishing.
bool hasBlockingIssue(List<RoomIssue> issues) =>
    issues.any((issue) => issue.severity == CheckSeverity.blocking);

/// A short suggestion of what a workable room looks like.
const String suggestedRoster =
    'A room that can finish work needs three jobs covered: someone to plan and '
    'deliver (Coordinator), someone to build (Engineer, Writer, Designer…), '
    'and someone to check (Reviewer, QA).';

/// Roles that fill a gap, for a "fix this" affordance.
List<RoleDef> rolesCovering(CheckSeverity _, String summary) {
  if (summary.contains('inspect')) {
    return Roles.inGroup(RoleGroup.quality);
  }
  if (summary.contains('deliver')) {
    return [Roles.byKey('coordinator')];
  }
  if (summary.contains('does the work')) {
    return Roles.inGroup(RoleGroup.building);
  }
  return const [];
}
