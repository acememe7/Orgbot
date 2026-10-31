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
    this.commands = const [],
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

  /// The programs that use it, to tell whether the tool is installed.
  final List<String> commands;

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
  '.local/share/orgbot',
  '.config/orgbot',
  '.local/share/com.orgbot.app',
  '.ssh',
  '.gnupg',
  '.codex',
  '.claude',
  '.claude.json',
  '.grok',
  '.gemini',
  '.config/opencode',
  '.local/share/opencode',
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
///
/// From the live room: the owner asked whether the app picks up the tools
/// and sign-ins on their computer. It knew thirteen; the rest had to be
/// added by hand.
List<LendEntry> lendCatalog() => [
  _known(
    'wrangler',
    'Cloudflare (Wrangler)',
    ['.config/.wrangler'],
    ['wrangler'],
  ),
  _known('gh', 'GitHub CLI', ['.config/gh'], ['gh']),
  _known('glab', 'GitLab CLI', ['.config/glab-cli'], ['glab']),
  _known('vercel', 'Vercel', ['.local/share/com.vercel.cli'], ['vercel']),
  _known('netlify', 'Netlify', ['.config/netlify'], ['netlify']),
  _known(
    'firebase',
    'Firebase CLI',
    ['.config/configstore/firebase-tools.json'],
    ['firebase'],
  ),
  _known('fly', 'Fly.io', ['.fly'], ['fly', 'flyctl']),
  _known('railway', 'Railway', ['.railway'], ['railway']),
  _known('doctl', 'DigitalOcean', ['.config/doctl'], ['doctl']),
  _known('supabase', 'Supabase', ['.supabase'], ['supabase']),
  _known('neon', 'Neon', ['.config/neonctl'], ['neonctl', 'neon']),
  _known('planetscale', 'PlanetScale', ['.config/planetscale'], ['pscale']),
  _known('turso', 'Turso', ['.config/turso'], ['turso']),
  _known('aws', 'AWS', ['.aws'], ['aws']),
  _known('gcloud', 'Google Cloud', ['.config/gcloud'], ['gcloud']),
  _known('azure', 'Azure CLI', ['.azure'], ['az']),
  _known('kube', 'Kubernetes', ['.kube'], ['kubectl']),
  _known('docker', 'Docker Hub', ['.docker/config.json'], ['docker']),
  _known(
    'terraform',
    'Terraform Cloud',
    ['.terraform.d/credentials.tfrc.json'],
    ['terraform'],
  ),
  _known('pulumi', 'Pulumi', ['.pulumi/credentials.json'], ['pulumi']),
  _known('expo', 'Expo (EAS)', ['.expo/state.json'], ['eas', 'expo']),
  _known('sentry', 'Sentry CLI', ['.sentryclirc'], ['sentry-cli']),
  _known('ngrok', 'ngrok', ['.config/ngrok'], ['ngrok']),
  _known('twilio', 'Twilio CLI', ['.twilio-cli'], ['twilio']),
  _known(
    'huggingface',
    'Hugging Face',
    ['.cache/huggingface/token'],
    ['huggingface-cli', 'hf'],
  ),
  _known('databricks', 'Databricks', ['.databrickscfg'], ['databricks']),
  _known('snowflake', 'Snowflake CLI', ['.snowflake'], ['snow']),
  _known('stripe', 'Stripe', ['.config/stripe'], ['stripe'], LendRisk.critical),
  _known('npm', 'npm publishing', ['.npmrc'], ['npm'], LendRisk.critical),
  _known('pypi', 'PyPI publishing', ['.pypirc'], ['twine'], LendRisk.critical),
  _known(
    'cargo',
    'crates.io publishing',
    ['.cargo/credentials.toml'],
    ['cargo'],
    LendRisk.critical,
  ),
  _known(
    'rubygems',
    'RubyGems publishing',
    ['.gem/credentials'],
    ['gem'],
    LendRisk.critical,
  ),
  _known(
    'rclone',
    'rclone remotes',
    ['.config/rclone/rclone.conf'],
    ['rclone'],
    LendRisk.critical,
  ),
];

LendEntry _known(
  String name,
  String label,
  List<String> paths,
  List<String> commands, [
  LendRisk risk = LendRisk.risky,
]) => LendEntry(
  name: name,
  label: label,
  paths: paths,
  commands: commands,
  risk: risk,
  builtIn: true,
);

/// Your sign-ins bots may ask to use, kept in OrgBots' data folder, which no
/// bot can read. Rooms read the list when they start.
class LendSettings {
  LendSettings(
    this.entries, {
    required this.file,
    required this.home,
    this.path = '',
  });

  final List<LendEntry> entries;
  final File file;
  final String home;

  /// The owner's PATH, to tell which tools are installed.
  final String path;

  /// The entries a room offers bots.
  List<LendEntry> get lent => entries
      .where((entry) => entry.enabled && entry.paths.isNotEmpty)
      .toList();

  /// Every listed path that is switched off: hidden from bots, never lent.
  ///
  /// From the live room: a sign-in the owner added and then switched off was
  /// left out of the room file, so nothing hid it.
  List<String> get hidden {
    final paths = <String>{
      for (final entry in entries)
        if (!entry.enabled)
          for (final path in entry.paths)
            if (lendPathProblem(path) == null) path,
    };
    return paths.toList()..sort();
  }

  /// Whether `entry`'s sign-in is on this computer.
  bool isPresent(LendEntry entry) => entry.paths.any(
    (path) =>
        FileSystemEntity.typeSync('$home/$path') !=
        FileSystemEntityType.notFound,
  );

  /// Whether one of the programs that use `entry` is on your PATH.
  bool isInstalled(LendEntry entry) {
    for (final dir in path.split(':')) {
      if (!dir.startsWith('/')) continue;
      for (final command in entry.commands) {
        final file = File('$dir/$command');
        try {
          if (file.statSync().type == FileSystemEntityType.file &&
              file.statSync().mode & 0x49 != 0) {
            return true;
          }
        } on FileSystemException {
          // Not here.
        }
      }
    }
    return false;
  }

  /// Sign-ins on this computer that are not turned on yet.
  List<LendEntry> get found =>
      entries.where((entry) => !entry.enabled && isPresent(entry)).toList();

  /// Tools installed here whose sign-in was not found: not signed in yet.
  List<LendEntry> get installedNotSignedIn => entries
      .where(
        (entry) => !entry.enabled && !isPresent(entry) && isInstalled(entry),
      )
      .toList();

  /// Add a sign-in a bot asked for and you allowed, turned on, keeping any
  /// entry of that name you already have. Returns whether it was added.
  bool addApproved(LendEntry entry) {
    if (!lendNamePattern.hasMatch(entry.name) ||
        entry.paths.isEmpty ||
        entry.paths.any((path) => lendPathProblem(path) != null)) {
      return false;
    }
    for (final existing in entries) {
      if (existing.name == entry.name) {
        if (existing.enabled) return false;
        existing.enabled = true;
        return true;
      }
    }
    entries.add(entry..enabled = true);
    return true;
  }

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
      saved.values.where(
        (entry) => entry.paths.every((path) => lendPathProblem(path) == null),
      ),
    );
    return LendSettings(
      entries,
      file: file,
      home: home,
      path: env['PATH'] ?? '',
    );
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
