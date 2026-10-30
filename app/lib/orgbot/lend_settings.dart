import 'dart:convert';
import 'dart:io';

/// How much harm using a lent sign-in could do. The values are the room
/// file's `risk` names.
enum LendRisk {
  regular('regular', 'Regular'),
  risky('risky', 'Risky'),
  critical('critical', 'Critical');

  const LendRisk(this.wire, this.label);
  final String wire;
  final String label;

  static LendRisk parse(Object? raw) =>
      values.firstWhere((risk) => risk.wire == raw, orElse: () => risky);
}

/// One of your sign-ins that bots may ask to use for a task.
class LendEntry {
  LendEntry({
    required this.name,
    required this.label,
    required this.paths,
    this.risk = LendRisk.risky,
    this.enabled = false,
    this.builtIn = false,
  });

  /// What bots ask for, such as `wrangler`.
  final String name;

  /// What you call it, such as "Cloudflare (Wrangler)".
  final String label;

  /// Where the sign-in lives, relative to your home folder.
  final List<String> paths;

  LendRisk risk;

  /// Whether bots may ask for it at all.
  bool enabled;

  /// One of the tools OrgBots knows, rather than one you added.
  final bool builtIn;

  Map<String, Object?> toJson() => {
    'name': name,
    'label': label,
    'paths': paths,
    'risk': risk.wire,
    'enabled': enabled,
  };

  static LendEntry? fromJson(Map<String, Object?> json) {
    final name = json['name'];
    final paths = json['paths'];
    if (name is! String || paths is! List) return null;
    return LendEntry(
      name: name,
      label: json['label'] as String? ?? name,
      paths: paths.whereType<String>().toList(),
      risk: LendRisk.parse(json['risk']),
      enabled: json['enabled'] == true,
    );
  }
}

/// A name bots can ask for: lowercase letters, digits, - and _.
final lendNamePattern = RegExp(r'^[a-z0-9_-]{1,40}$');

/// Paths OrgBots never lends, whatever is listed: its own data, private keys
/// and the model CLIs' sign-ins (bots get their own copies of those). The
/// room service and the sandbox launcher check this again.
const neverLent = [
  '.local/share/orgbot', '.config/orgbot', '.local/share/com.orgbot.app',
  '.ssh', '.gnupg', '.codex', '.claude', '.claude.json', '.grok',
  '.gemini', '.config/opencode', '.local/share/opencode',
];

/// Why `path` cannot be lent, or null when it can.
String? lendPathProblem(String path) {
  final parts = path.split('/');
  if (path.isEmpty ||
      path.startsWith('/') ||
      parts.any((part) => part.isEmpty || part == '.' || part == '..')) {
    return 'Use a path inside your home folder, such as .config/tool';
  }
  for (final never in neverLent) {
    if (path == never ||
        path.startsWith('$never/') ||
        never.startsWith('$path/')) {
      return 'OrgBots never lends this to bots';
    }
  }
  return null;
}

/// Tools OrgBots knows where to find. Off until you turn them on.
List<LendEntry> lendCatalog() => [
  LendEntry(name: 'wrangler', label: 'Cloudflare (Wrangler)', paths: ['.config/.wrangler'], builtIn: true),
  LendEntry(name: 'gh', label: 'GitHub CLI', paths: ['.config/gh'], builtIn: true),
  LendEntry(name: 'vercel', label: 'Vercel', paths: ['.local/share/com.vercel.cli'], builtIn: true),
  LendEntry(name: 'netlify', label: 'Netlify', paths: ['.config/netlify'], builtIn: true),
  LendEntry(name: 'firebase', label: 'Firebase CLI', paths: ['.config/configstore/firebase-tools.json'], builtIn: true),
  LendEntry(name: 'fly', label: 'Fly.io', paths: ['.fly'], builtIn: true),
  LendEntry(name: 'doctl', label: 'DigitalOcean', paths: ['.config/doctl'], builtIn: true),
  LendEntry(name: 'supabase', label: 'Supabase', paths: ['.supabase'], builtIn: true),
  LendEntry(name: 'aws', label: 'AWS', paths: ['.aws'], builtIn: true),
  LendEntry(name: 'gcloud', label: 'Google Cloud', paths: ['.config/gcloud'], builtIn: true),
  LendEntry(name: 'kube', label: 'Kubernetes', paths: ['.kube'], builtIn: true),
  LendEntry(name: 'stripe', label: 'Stripe', paths: ['.config/stripe'], risk: LendRisk.critical, builtIn: true),
  LendEntry(name: 'npm', label: 'npm publishing', paths: ['.npmrc'], risk: LendRisk.critical, builtIn: true),
];

/// Your sign-ins bots may ask to use, kept in OrgBots' data folder, which no
/// bot can read. Rooms read the list when they start.
class LendSettings {
  LendSettings(this.entries, {required this.file, required this.home});

  final List<LendEntry> entries;
  final File file;
  final String home;

  /// The entries a room offers bots.
  List<LendEntry> get lent =>
      entries.where((entry) => entry.enabled && entry.paths.isNotEmpty).toList();

  /// Whether `entry`'s sign-in is on this computer.
  bool isPresent(LendEntry entry) => entry.paths.any(
    (path) => FileSystemEntity.typeSync('$home/$path') != FileSystemEntityType.notFound,
  );

  static String _dataHome(Map<String, String> environment, String home) {
    final xdg = environment['XDG_DATA_HOME'];
    return xdg != null && xdg.startsWith('/') ? xdg : '$home/.local/share';
  }

  /// Load the list, with every known tool present and your choices applied.
  static LendSettings load({Map<String, String>? environment}) {
    final env = environment ?? Platform.environment;
    final home = env['HOME'] ?? '';
    final file = File('${_dataHome(env, home)}/orgbot/lending.json');
    final saved = <String, LendEntry>{};
    try {
      final decoded = jsonDecode(file.readAsStringSync());
      if (decoded is Map && decoded['lend'] is List) {
        for (final raw in decoded['lend'] as List) {
          if (raw is! Map) continue;
          final entry = LendEntry.fromJson(raw.cast<String, Object?>());
          if (entry != null && lendNamePattern.hasMatch(entry.name)) {
            saved[entry.name] = entry;
          }
        }
      }
    } on Object {
      // Missing or unreadable: every tool starts off.
    }
    final entries = <LendEntry>[];
    for (final known in lendCatalog()) {
      final choice = saved.remove(known.name);
      if (choice != null) {
        known
          ..enabled = choice.enabled
          ..risk = choice.risk;
      }
      entries.add(known);
    }
    entries.addAll(
      saved.values.where((entry) => entry.paths.every((path) => lendPathProblem(path) == null)),
    );
    return LendSettings(entries, file: file, home: home);
  }

  void save() {
    file.parent.createSync(recursive: true);
    final temporary = File('${file.path}.tmp');
    temporary.writeAsStringSync(
      const JsonEncoder.withIndent('  ').convert({
        'schema': 1,
        'lend': entries.map((entry) => entry.toJson()).toList(),
      }),
    );
    temporary.renameSync(file.path);
  }
}
