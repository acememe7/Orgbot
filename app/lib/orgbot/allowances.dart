import 'dart:convert';
import 'dart:io';

import 'package:path_provider/path_provider.dart';

class ProviderAllowance {
  const ProviderAllowance({
    this.windowSeconds = 604800,
    this.requests = 0,
    this.tokens = 0,
    this.costMicrousd = 0,
    this.inputPrice = 0,
    this.outputPrice = 0,
    this.outputTokens = 16000,
    this.enabled = true,
  });
  final int windowSeconds,
      requests,
      tokens,
      costMicrousd,
      inputPrice,
      outputPrice,
      outputTokens;
  final bool enabled;

  factory ProviderAllowance.fromJson(Map<String, dynamic> value) {
    int read(String key, [int fallback = 0]) {
      final field = value[key];
      if (field == null) return fallback;
      if (field is! int || field < 0 || field > 9007199254740991) {
        throw FormatException('Invalid allowance field: $key');
      }
      return field;
    }

    final result = ProviderAllowance(
      windowSeconds: read('window_secs', 604800),
      requests: read('max_requests'),
      tokens: read('max_tokens'),
      costMicrousd: read('max_cost_microusd'),
      inputPrice: read('input_microusd_per_million'),
      outputPrice: read('output_microusd_per_million'),
      outputTokens: read('max_output_tokens', 16000),
      enabled: value['enabled'] != false,
    );
    result.validate();
    return result;
  }

  void validate() {
    if (windowSeconds < 60 ||
        windowSeconds > 31536000 ||
        outputTokens < 1 ||
        outputTokens > 128000 ||
        [
          requests,
          tokens,
          costMicrousd,
          inputPrice,
          outputPrice,
        ].any((v) => v < 0 || v > 9007199254740991)) {
      throw const FormatException('Invalid spending limit');
    }
    if (costMicrousd > 0 && (inputPrice == 0 || outputPrice == 0)) {
      throw const FormatException(
        'Enter both token prices before setting a money limit',
      );
    }
  }

  Map<String, Object> toJson() => {
    'window_secs': windowSeconds,
    'max_requests': requests,
    'max_tokens': tokens,
    'max_cost_microusd': costMicrousd,
    'input_microusd_per_million': inputPrice,
    'output_microusd_per_million': outputPrice,
    'max_output_tokens': outputTokens,
    'enabled': enabled,
  };
}

class DesktopAllowances {
  DesktopAllowances(this.directory);
  final Directory directory;
  File get _config => File('${directory.path}/policies.json');
  File get _ledger => File('${directory.path}/ledger.json');

  static Future<DesktopAllowances> open() async => DesktopAllowances(
    Directory('${(await getApplicationSupportDirectory()).path}/allowances'),
  );

  static Future<Map<String, String>> environment({
    Iterable<String> projectIds = const [],
  }) async {
    final manager = await open();
    await manager._prepare();
    return {
      'ORGBOT_ALLOWANCE_CONFIG': manager._config.path,
      'ORGBOT_ALLOWANCE_LEDGER': manager._ledger.path,
      'ORGBOT_MEMORY_ROOT': '${manager.directory.parent.path}/project-memory',
      'ORGBOT_MEMORY_PROJECTS': jsonEncode(projectIds.toSet().toList()),
    };
  }

  Future<void> _prepare() async {
    if (await FileSystemEntity.type(directory.path, followLinks: false) ==
        FileSystemEntityType.link) {
      throw const FileSystemException(
        'Spending controls directory cannot be a symbolic link',
      );
    }
    await directory.create(recursive: true);
    if (Platform.isLinux) {
      final result = await Process.run('chmod', ['700', directory.path]);
      if (result.exitCode != 0) {
        throw const FileSystemException('Cannot protect spending controls');
      }
    }
  }

  Future<Map<String, dynamic>> _read(File file) async {
    final type = await FileSystemEntity.type(file.path, followLinks: false);
    if (type == FileSystemEntityType.notFound) return {};
    if (type != FileSystemEntityType.file ||
        await file.length() > 1024 * 1024) {
      throw const FormatException(
        'Spending data is not a bounded regular file',
      );
    }
    final data = jsonDecode(await file.readAsString());
    if (data is! Map<String, dynamic>) {
      throw const FormatException('Invalid spending data');
    }
    return data;
  }

  Future<Map<String, ProviderAllowance>> policies() async {
    final data = await _read(_config);
    final providers = data['providers'];
    if (providers == null) return {};
    if (providers is! Map<String, dynamic>) {
      throw const FormatException('Invalid provider limits');
    }
    return providers.map((key, value) {
      if (value is! Map<String, dynamic>) {
        throw const FormatException('Invalid provider limit');
      }
      return MapEntry(key, ProviderAllowance.fromJson(value));
    });
  }

  Future<Map<String, dynamic>> usage() async {
    final providers = (await _read(_ledger))['providers'];
    if (providers == null) return {};
    if (providers is! Map<String, dynamic>) {
      throw const FormatException('Invalid usage ledger');
    }
    return providers;
  }

  Future<void> save(String provider, ProviderAllowance policy) async {
    policy.validate();
    await _prepare();
    final lockFile = File('${directory.path}/policies.lock');
    final lockType = await FileSystemEntity.type(
      lockFile.path,
      followLinks: false,
    );
    if (lockType != FileSystemEntityType.file &&
        lockType != FileSystemEntityType.notFound) {
      throw const FileSystemException('Spending lock must be a regular file');
    }
    final lock = await lockFile.open(mode: FileMode.append);
    await lock.lock(FileLock.blockingExclusive);
    File? temporary;
    try {
      final values = await policies();
      values[provider] = policy;
      temporary = File(
        '${_config.path}.${pid}_${DateTime.now().microsecondsSinceEpoch}.tmp',
      );
      await temporary.writeAsString(
        jsonEncode({
          'providers': values.map(
            (key, value) => MapEntry(key, value.toJson()),
          ),
        }),
        flush: true,
      );
      if (Platform.isLinux) {
        final result = await Process.run('chmod', ['600', temporary.path]);
        if (result.exitCode != 0) {
          throw const FileSystemException('Cannot protect spending settings');
        }
      }
      await temporary.rename(_config.path);
      temporary = null;
    } finally {
      if (temporary != null && await temporary.exists()) {
        await temporary.delete();
      }
      await lock.unlock();
      await lock.close();
    }
  }
}
