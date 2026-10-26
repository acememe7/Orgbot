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
    this.cacheReadPrice,
    this.cacheWritePrice,
    this.modelRates = const {},
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
  final int? cacheReadPrice, cacheWritePrice;
  final Map<String, ModelTokenRates> modelRates;
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
      cacheReadPrice: value['cache_read_microusd_per_million'] == null
          ? null
          : read('cache_read_microusd_per_million'),
      cacheWritePrice: value['cache_write_microusd_per_million'] == null
          ? null
          : read('cache_write_microusd_per_million'),
      modelRates: _readModelRates(value['model_rates']),
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
          cacheReadPrice ?? 0,
          cacheWritePrice ?? 0,
        ].any((v) => v < 0 || v > 9007199254740991)) {
      throw const FormatException('Invalid spending limit');
    }
    if (costMicrousd > 0 &&
        modelRates.isEmpty &&
        (inputPrice == 0 || outputPrice == 0)) {
      throw const FormatException(
        'Enter both token prices before setting a money limit',
      );
    }
    for (final entry in modelRates.entries) {
      if (entry.key.trim().isEmpty ||
          entry.value.values.any((v) => v < 0 || v > 9007199254740991)) {
        throw const FormatException('Invalid model token rates');
      }
    }
  }

  Map<String, Object> toJson() => {
    'window_secs': windowSeconds,
    'max_requests': requests,
    'max_tokens': tokens,
    'max_cost_microusd': costMicrousd,
    'input_microusd_per_million': inputPrice,
    'output_microusd_per_million': outputPrice,
    'cache_read_microusd_per_million': ?cacheReadPrice,
    'cache_write_microusd_per_million': ?cacheWritePrice,
    if (modelRates.isNotEmpty)
      'model_rates': modelRates.map(
        (key, value) => MapEntry(key, value.toJson()),
      ),
    'max_output_tokens': outputTokens,
    'enabled': enabled,
  };

  static Map<String, ModelTokenRates> _readModelRates(Object? raw) {
    if (raw == null) return const {};
    if (raw is! Map<String, dynamic>) {
      throw const FormatException('Invalid model rates');
    }
    return raw.map((model, value) {
      if (value is! Map<String, dynamic>) {
        throw const FormatException('Invalid model rate');
      }
      return MapEntry(model, ModelTokenRates.fromJson(value));
    });
  }
}

class ModelTokenRates {
  const ModelTokenRates(
    this.input,
    this.output,
    this.cacheRead,
    this.cacheWrite,
  );
  final int input, output, cacheRead, cacheWrite;
  List<int> get values => [input, output, cacheRead, cacheWrite];
  factory ModelTokenRates.fromJson(Map<String, dynamic> json) {
    int rate(String key) {
      final value = json[key];
      if (value is! int || value < 0 || value > 9007199254740991) {
        throw FormatException('Invalid model rate: $key');
      }
      return value;
    }

    return ModelTokenRates(
      rate('input_microusd_per_million'),
      rate('output_microusd_per_million'),
      rate('cache_read_microusd_per_million'),
      rate('cache_write_microusd_per_million'),
    );
  }
  Map<String, int> toJson() => {
    'input_microusd_per_million': input,
    'output_microusd_per_million': output,
    'cache_read_microusd_per_million': cacheRead,
    'cache_write_microusd_per_million': cacheWrite,
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
