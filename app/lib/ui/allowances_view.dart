import 'package:flutter/material.dart';
import '../orgbot/allowances.dart';

class AllowancesView extends StatefulWidget {
  const AllowancesView({super.key, this.manager});
  final DesktopAllowances? manager;
  @override
  State<AllowancesView> createState() => _AllowancesViewState();
}

class _AllowancesViewState extends State<AllowancesView> {
  static const providers = {
    'cli:codex': 'Codex CLI',
    'cli:claude': 'Claude Code',
    'cli:grok': 'Grok CLI',
    'cli:opencode': 'OpenCode CLI',
    'cli:agy': 'Antigravity CLI',
    'deepseek': 'DeepSeek API',
    'openai:ollama': 'Ollama API',
  };
  DesktopAllowances? _manager;
  Map<String, ProviderAllowance> _policies = {};
  Map<String, dynamic> _usage = {};
  bool _loading = false;
  String? _error;

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      _manager ??= widget.manager ?? await DesktopAllowances.open();
      final policies = await _manager!.policies();
      final usage = await _manager!.usage();
      if (mounted) {
        setState(() {
          _policies = policies;
          _usage = usage;
        });
      }
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) => ExpansionTile(
    title: const Text('Spending controls'),
    leading: const Icon(Icons.tune),
    onExpansionChanged: (open) {
      if (open) _load();
    },
    children: [
      if (_loading) const LinearProgressIndicator(),
      if (_error != null)
        ListTile(
          title: Text(_error!),
          trailing: TextButton(onPressed: _load, child: const Text('Retry')),
        ),
      for (final provider in providers.entries)
        ListTile(
          title: Text(provider.value),
          subtitle: Text(
            _status(provider.key),
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
          ),
          trailing: IconButton(
            tooltip: 'Edit ${provider.value} allowance',
            onPressed: _loading ? null : () => _edit(provider.key),
            icon: const Icon(Icons.edit_outlined),
          ),
          onTap: _loading ? null : () => _edit(provider.key),
        ),
      Align(
        alignment: Alignment.centerRight,
        child: TextButton.icon(
          onPressed: _loading ? null : _load,
          icon: const Icon(Icons.refresh),
          label: const Text('Refresh usage'),
        ),
      ),
    ],
  );

  String _status(String key) {
    final policy = _policies[key] ?? const ProviderAllowance();
    if (!policy.enabled) return 'Disabled';
    final usage = _usage[key];
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final start = usage is Map<String, dynamic> ? usage['window_start'] : null;
    if (usage is Map<String, dynamic> &&
        start is int &&
        now < start + policy.windowSeconds) {
      final message = usage['message'];
      if (message is String && message.isNotEmpty) return message;
      final count = usage['requests'];
      if (count is int) {
        return '$count requests${policy.requests > 0 ? ' / ${policy.requests}' : ''} · ${policy.windowSeconds == 86400 ? 'daily' : 'weekly'} window';
      }
    }
    return policy.requests > 0
        ? '${policy.requests} requests per ${policy.windowSeconds == 86400 ? 'day' : 'week'}'
        : 'No local cap';
  }

  Future<void> _edit(String key) async {
    final current = _policies[key] ?? const ProviderAllowance();
    final policy = await showDialog<ProviderAllowance>(
      context: context,
      builder: (context) => _AllowanceDialog(
        label: providers[key]!,
        cli: key.startsWith('cli:'),
        policy: current,
      ),
    );
    if (policy == null) return;
    try {
      _manager ??= widget.manager ?? await DesktopAllowances.open();
      await _manager!.save(key, policy);
      if (mounted) await _load();
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }
}

class _AllowanceDialog extends StatefulWidget {
  const _AllowanceDialog({
    required this.label,
    required this.cli,
    required this.policy,
  });
  final String label;
  final bool cli;
  final ProviderAllowance policy;
  @override
  State<_AllowanceDialog> createState() => _AllowanceDialogState();
}

class _AllowanceDialogState extends State<_AllowanceDialog> {
  final _form = GlobalKey<FormState>();
  late final _requests = TextEditingController(
    text: '${widget.policy.requests}',
  );
  late final _tokens = TextEditingController(text: '${widget.policy.tokens}');
  late final _cost = TextEditingController(
    text: '${widget.policy.costMicrousd / 1000000}',
  );
  late final _input = TextEditingController(
    text: '${widget.policy.inputPrice / 1000000}',
  );
  late final _output = TextEditingController(
    text: '${widget.policy.outputPrice / 1000000}',
  );
  late final _cacheRead = TextEditingController(
    text:
        '${(widget.policy.cacheReadPrice ?? widget.policy.inputPrice) / 1000000}',
  );
  late final _cacheWrite = TextEditingController(
    text:
        '${(widget.policy.cacheWritePrice ?? widget.policy.inputPrice) / 1000000}',
  );
  late final _modelRates = TextEditingController(
    text: widget.policy.modelRates.entries
        .map(
          (entry) =>
              '${entry.key},${entry.value.input / 1000000},${entry.value.output / 1000000},${entry.value.cacheRead / 1000000},${entry.value.cacheWrite / 1000000}',
        )
        .join('\n'),
  );
  late final _ceiling = TextEditingController(
    text: '${widget.policy.outputTokens}',
  );
  late int _window = widget.policy.windowSeconds;
  late bool _enabled = widget.policy.enabled;
  String? _error;
  @override
  void dispose() {
    for (final c in [
      _requests,
      _tokens,
      _cost,
      _input,
      _output,
      _cacheRead,
      _cacheWrite,
      _modelRates,
      _ceiling,
    ]) {
      c.dispose();
    }
    super.dispose();
  }

  Widget _number(
    String label,
    TextEditingController controller, {
    bool money = false,
  }) => Padding(
    padding: const EdgeInsets.only(bottom: 14),
    child: TextFormField(
      controller: controller,
      decoration: InputDecoration(labelText: label),
      keyboardType: TextInputType.number,
      validator: (value) {
        final number = money
            ? double.tryParse(value ?? '')
            : int.tryParse(value ?? '');
        return number == null ||
                !number.isFinite ||
                number < 0 ||
                number > 1000000000
            ? 'Enter a nonnegative number up to 1 billion'
            : null;
      },
    ),
  );

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.label),
    content: SizedBox(
      width: 440,
      child: SingleChildScrollView(
        child: Form(
          key: _form,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                title: const Text('Provider enabled'),
                value: _enabled,
                onChanged: (value) => setState(() => _enabled = value),
              ),
              DropdownButtonFormField<int>(
                initialValue: _window,
                decoration: const InputDecoration(
                  labelText: 'Allowance window',
                ),
                items: [
                  if (_window != 86400 && _window != 604800)
                    DropdownMenuItem(
                      value: _window,
                      child: Text("Custom ($_window seconds)"),
                    ),
                  const DropdownMenuItem(
                    value: 86400,
                    child: Text('Daily (UTC)'),
                  ),
                  const DropdownMenuItem(
                    value: 604800,
                    child: Text('Weekly (UTC)'),
                  ),
                ],
                onChanged: (value) => setState(() => _window = value!),
              ),
              const SizedBox(height: 18),
              _number(
                widget.cli
                    ? 'CLI episodes (0 = unlimited)'
                    : 'API requests (0 = unlimited)',
                _requests,
              ),
              if (widget.cli) ...[
                const Text(
                  'CLI caps count episodes only. Charges depend on the selected CLI provider and account. API-equivalent prices are estimates; OrgBots does not enforce CLI dollar or token caps.',
                ),
                const SizedBox(height: 14),
              ]
              else ...[
                _number('Tokens (0 = unlimited)', _tokens),
                _number('USD (0 = unlimited)', _cost, money: true),
                _number('Maximum output tokens per request', _ceiling),
                const Text(
                  'Token and money limits reserve each text request before sending. Enter current provider prices. Image work requires a request limit; unknown usage keeps its reservation.',
                ),
              ],
              _number('Input USD per million tokens', _input, money: true),
              _number('Output USD per million tokens', _output, money: true),
              _number(
                'Cache read USD per million tokens',
                _cacheRead,
                money: true,
              ),
              _number(
                'Cache write USD per million tokens',
                _cacheWrite,
                money: true,
              ),
              TextFormField(
                controller: _modelRates,
                minLines: 2,
                maxLines: 6,
                decoration: const InputDecoration(
                  labelText: 'Model rates',
                  hintText:
                      'model,input,output,cache read,cache write (USD per million)',
                ),
                validator: _validateModelRates,
              ),
              if (_error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
                    _error!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(onPressed: _save, child: const Text('Save')),
    ],
  );

  void _save() {
    if (!_form.currentState!.validate()) return;
    try {
      final policy = ProviderAllowance(
        windowSeconds: _window,
        requests: int.parse(_requests.text),
        tokens: widget.cli ? 0 : int.parse(_tokens.text),
        costMicrousd: widget.cli
            ? 0
            : (double.parse(_cost.text) * 1000000).round(),
        inputPrice: (double.parse(_input.text) * 1000000).round(),
        outputPrice: (double.parse(_output.text) * 1000000).round(),
        cacheReadPrice: (double.parse(_cacheRead.text) * 1000000).round(),
        cacheWritePrice: (double.parse(_cacheWrite.text) * 1000000).round(),
        modelRates: _parseModelRates(_modelRates.text),
        outputTokens: widget.cli ? 8192 : int.parse(_ceiling.text),
        enabled: _enabled,
      );
      policy.validate();
      Navigator.pop(context, policy);
    } catch (error) {
      setState(() => _error = '$error');
    }
  }

  String? _validateModelRates(String? raw) {
    try {
      _parseModelRates(raw ?? '');
      return null;
    } on FormatException catch (error) {
      return error.message;
    }
  }

  Map<String, ModelTokenRates> _parseModelRates(String raw) {
    final rates = <String, ModelTokenRates>{};
    for (final line
        in raw
            .split('\n')
            .map((line) => line.trim())
            .where((line) => line.isNotEmpty)) {
      final fields = line.split(',').map((field) => field.trim()).toList();
      if (fields.length != 5 ||
          fields.first.isEmpty ||
          rates.containsKey(fields.first)) {
        throw const FormatException(
          'Use one unique model,input,output,cache read,cache write row per model',
        );
      }
      final prices = fields
          .skip(1)
          .map((field) => double.tryParse(field))
          .toList();
      if (prices.any(
        (price) =>
            price == null || !price.isFinite || price < 0 || price > 1000000000,
      )) {
        throw const FormatException(
          'Model rates must be nonnegative USD per million tokens',
        );
      }
      rates[fields.first] = ModelTokenRates(
        (prices[0]! * 1000000).round(),
        (prices[1]! * 1000000).round(),
        (prices[2]! * 1000000).round(),
        (prices[3]! * 1000000).round(),
      );
    }
    return rates;
  }
}
