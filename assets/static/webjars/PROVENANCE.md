# Vendored front-end libraries

The Java distribution pulled these libraries in as webjars. They are vendored here under the exact
URL paths the templates and the ShinyProxy JavaScript expect (`/webjars/<name>/<version>/...`), so
that the front-end code could be reused without modification.

| Path | Version | Source | SHA-256 |
| --- | --- | --- | --- |
| `datatables/1.13.5/css/jquery.dataTables.min.css` | 1.13.5 | https://cdn.datatables.net/1.13.5/css/jquery.dataTables.min.css | `5e9bf0ca99854ef5cde954de1b15f0410c38d658d8a8f9048003911aa6b36b26` |
| `datatables/1.13.5/js/jquery.dataTables.min.js` | 1.13.5 | https://cdn.datatables.net/1.13.5/js/jquery.dataTables.min.js | `4a20199d45c7b3b9180461baa8f93a383e0438ac921a8bbcef0c3ab5c986c1c3` |
| `datatables-buttons/2.4.1/js/dataTables.buttons.min.js` | 2.4.1 | https://cdn.datatables.net/buttons/2.4.1/js/dataTables.buttons.min.js | `d94ba2a088fb38d48267fa162d3c9b0fbd8d822aa5d593a5978cf9ce3a88443a` |
| `datatables-responsive/2.2.7/css/responsive.dataTables.min.css` | 2.2.7 | https://cdn.datatables.net/responsive/2.2.7/css/responsive.dataTables.min.css | `63f01d056d6786fccfa30b93d65bc5e0f918e9047e9ea63305c6e6903086df46` |
| `datatables-responsive/2.2.7/js/dataTables.responsive.min.js` | 2.2.7 | https://cdn.datatables.net/responsive/2.2.7/js/dataTables.responsive.min.js | `661e6bc13d34928b2752a139f3935b4d9399dd35bf9efe3d4d7cbd05d0e34b8a` |
| `handlebars/4.7.9/dist/handlebars.runtime.min.js` | 4.7.9 | https://cdn.jsdelivr.net/npm/handlebars@4.7.9/dist/handlebars.runtime.min.js | `7fd551e31d1fbfd98eb6e830479a5ee022eff76dc4ba4517b0f53d035390cbe5` |
| `jquery/3.7.1/jquery.min.js` | 3.7.1 | https://code.jquery.com/jquery-3.7.1.min.js | `fc9a93dd241f6b045cbff0481cf4e1901becd0e12fb45166a8f17f95823f0b1a` |

Regenerate hashes with:

```
cd assets/static/webjars && find . -type f -exec sha256sum {} +
```
