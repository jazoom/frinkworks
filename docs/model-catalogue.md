# Model catalogue

Frinkworks loads a bundled or local catalogue before the network refresh. It refreshes at startup and once a day.

A failed refresh keeps the last valid catalogue. Settings shows the last successful check and the last attempt. A failed attempt exposes Retry refresh.

## Model retirement

Deprecation does not invalidate a catalogue. The catalogue retains deprecated models and their capabilities, but new-model choices exclude them.

Ordinary new drafts use this order within the selected provider:

1. The saved model, if eligible.
2. The configured provider default, if eligible.
3. The eligible model with the first identifier in lexical order.

An eligible model supports tools and text input, produces text output, and is not deprecated. The fallback never changes providers.

A fallback notice names the replacement before the user sends a message. The fallback changes neither stored preferences nor saved conversations.

If the provider has no eligible model, new model requests cannot start with that provider. Other providers remain available.

Copied drafts retain their requested settings. A deprecated model requires an explicit replacement before a new conversation starts.

## Saved conversations

Saved conversations keep their exact model identifier. A deprecated selection shows a notice and a Choose model action. Deprecation alone does not block requests.

The provider decides whether it accepts requests for that identifier. Frinkworks does not substitute another model after a provider rejection.

## DeepSeek

The default is `deepseek-flash`. [DeepSeek’s model documentation](https://api-docs.deepseek.com/quick_start/pricing) lists this identifier and describes the older Flash names as accepted aliases.
