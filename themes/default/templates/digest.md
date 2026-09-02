{# Body of the daily digest issue. Markdown, rendered by GitHub. #}
**{{ digest.count }} new item{{ 's' if digest.count != 1 }}** since {{ digest.since | date('%Y-%m-%d %H:%M UTC') }}.
{% if digest.site_url %}
[feed]({{ digest.site_url }}) · [search]({{ digest.site_url }}search/) · [categories]({{ digest.site_url }}categories/) · [tags]({{ digest.site_url }}tags/)
{% endif %}

{% for group in groups %}
### {{ group.name }} ({{ group.items | length }})

{% for item in group.items %}
- [{{ item.title }}]({{ item.link }}){{ " · [source](" ~ item.page ~ ")" if item.page }}{{ " · [md](" ~ item.permalink ~ ")" if item.permalink }}
{% endfor %}

{% endfor %}
{% if digest.omitted %}
_…and {{ digest.omitted }} more{% if digest.site_url %} on the [feed]({{ digest.site_url }}){% endif %}._

{% endif %}
---
<sub>Digest #{{ digest.number }} · {{ digest.date }}{% if digest.data_sha %} · data@{{ digest.data_sha[:7] }}{% endif %} · <a href="https://github.com/aymericbeaumet/aggr">aggr</a></sub>
