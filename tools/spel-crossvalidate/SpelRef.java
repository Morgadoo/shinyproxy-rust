import org.springframework.expression.EvaluationContext;
import org.springframework.expression.Expression;
import org.springframework.expression.ExpressionParser;
import org.springframework.expression.ParserContext;
import org.springframework.expression.spel.standard.SpelExpressionParser;
import org.springframework.expression.spel.support.StandardEvaluationContext;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Evaluates the expressions of the Rust corpus with the real Spring Expression Language, so the two
 * implementations can be compared. Reads one expression per line, prints "expression\tresult".
 */
public class SpelRef {

    /** Mimics eu.openanalytics.containerproxy.spec.expression.SpecExpressionContext. */
    public static class Root {
        public final ProxyObj proxy;
        public final SpecObj proxySpec;
        public final String userId;
        public final List<String> groups;
        public final UserObj oidcUser;
        public final UserObj ldapUser;
        public final String serverName;

        Root(ProxyObj proxy, SpecObj proxySpec, String userId, List<String> groups,
             UserObj oidcUser, UserObj ldapUser, String serverName) {
            this.proxy = proxy;
            this.proxySpec = proxySpec;
            this.userId = userId;
            this.groups = groups;
            this.oidcUser = oidcUser;
            this.ldapUser = ldapUser;
            this.serverName = serverName;
        }

        public ProxyObj getProxy() { return proxy; }
        public SpecObj getProxySpec() { return proxySpec; }
        public String getUserId() { return userId; }
        public List<String> getGroups() { return groups; }
        public UserObj getOidcUser() { return oidcUser; }
        public UserObj getLdapUser() { return ldapUser; }
        public String getServerName() { return serverName; }

        public List<String> toList(String attribute, String regex) {
            if (attribute == null) return List.of();
            return Arrays.stream(attribute.split(regex)).map(String::trim).toList();
        }

        public List<String> toList(String attribute) { return toList(attribute, ","); }

        public List<String> toLowerCaseList(String attribute, String regex) {
            if (attribute == null) return List.of();
            return Arrays.stream(attribute.split(regex)).map(s -> s.trim().toLowerCase()).toList();
        }

        public List<String> toLowerCaseList(String attribute) { return toLowerCaseList(attribute, ","); }

        public List<String> toLowerCaseList(List<String> values) {
            if (values == null) return List.of();
            return values.stream().map(v -> v.trim().toLowerCase()).toList();
        }

        public List<String> toList(List<String> values) {
            if (values == null) return List.of();
            return values.stream().map(String::trim).toList();
        }

        public boolean isOneOf(String attribute, String... allowedValues) {
            if (attribute == null) return false;
            return Arrays.stream(allowedValues).anyMatch(it -> it.trim().equals(attribute.trim()));
        }

        public boolean isOneOfIgnoreCase(String attribute, String... allowedValues) {
            if (attribute == null) return false;
            return Arrays.stream(allowedValues).anyMatch(it -> it.trim().equalsIgnoreCase(attribute.trim()));
        }
    }

    /** Stands in for CustomNameOidcUser / LdapUserDetails: attributes are a bean property. */
    public static class UserObj {
        private final Map<String, Object> attributes;
        UserObj(Map<String, Object> attributes) { this.attributes = attributes; }
        public Map<String, Object> getAttributes() { return attributes; }
        public Map<String, Object> getClaims() { return attributes; }
        public String getEmail() { return (String) attributes.get("email"); }
        public String getName() { return "jack"; }
    }

    /** Stands in for ProxySpec. */
    public static class SpecObj {
        private final String id;
        private final String displayName;
        SpecObj(String id, String displayName) { this.id = id; this.displayName = displayName; }
        public String getId() { return id; }
        public String getDisplayName() { return displayName; }
    }

    public static class ProxyObj {
        private final String id;
        private final String userId;
        private final String specId;
        private final Map<String, String> runtimeValues;

        ProxyObj(String id, String userId, String specId, Map<String, String> runtimeValues) {
            this.id = id;
            this.userId = userId;
            this.specId = specId;
            this.runtimeValues = runtimeValues;
        }

        public String getId() { return id; }
        public String getUserId() { return userId; }
        public String getSpecId() { return specId; }
        public String getStatus() { return "Up"; }
        public String getRuntimeValue(String key) { return runtimeValues.get(key); }
        public String getRuntimeValueOrDefault(String key, String defaultValue) {
            return runtimeValues.getOrDefault(key, defaultValue);
        }
        @Override public String toString() { return id; }
    }

    public static void main(String[] args) throws Exception {
        Map<String, Object> oidcAttributes = new LinkedHashMap<>();
        oidcAttributes.put("dept", "research");
        oidcAttributes.put("email", "jack@example.com");
        oidcAttributes.put("groups", List.of("scientists", "admins"));
        oidcAttributes.put("memberOf", "Research, Data Science");
        oidcAttributes.put("quota", 4);

        UserObj oidcUser = new UserObj(oidcAttributes);

        Map<String, Object> ldapAttributes = new LinkedHashMap<>();
        ldapAttributes.put("memberOf", List.of("cn=scientists,dc=example"));
        UserObj ldapUser = new UserObj(ldapAttributes);

        SpecObj proxySpec = new SpecObj("01_hello", "Hello Application");

        Map<String, String> runtimeValues = new HashMap<>();
        runtimeValues.put("SHINYPROXY_USERNAME", "jack");
        runtimeValues.put("SHINYPROXY_USERGROUPS", "SCIENTISTS,MATHEMATICIANS");
        runtimeValues.put("SHINYPROXY_PARAMETERS", "{\"resources\":\"2-8\"}");

        Root root = new Root(
            new ProxyObj("5f39a7cf-c9ff-4a85-9313-d561ec79cca9", "jack", "01_hello", runtimeValues),
            proxySpec,
            "jack",
            new ArrayList<>(List.of("SCIENTISTS", "MATHEMATICIANS")),
            oidcUser,
            ldapUser,
            "shinyproxy.example.com");

        ExpressionParser parser = new SpelExpressionParser();
        ParserContext templateContext = ParserContext.TEMPLATE_EXPRESSION;
        EvaluationContext context = new StandardEvaluationContext(root);

        List<String> lines = Files.readAllLines(Paths.get(args[0]), StandardCharsets.UTF_8);
        for (String line : lines) {
            if (line.isEmpty()) {
                continue;
            }
            String expression = line;
            String result;
            try {
                Expression parsed = parser.parseExpression(expression, templateContext);
                Object value = parsed.getValue(context);
                result = String.valueOf(value);
            } catch (Throwable throwable) {
                Throwable cause = throwable;
                while (cause.getCause() != null) {
                    cause = cause.getCause();
                }
                result = "ERROR: " + cause.getClass().getSimpleName() + ": " + cause.getMessage();
            }
            System.out.println(expression + "\t" + result.replace("\n", "\\n"));
        }
    }
}
