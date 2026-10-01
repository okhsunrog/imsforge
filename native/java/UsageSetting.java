import java.lang.reflect.InvocationTargetException;

/** Uses the framework's named Binder API; no Android-version-specific transaction IDs. */
public final class UsageSetting {
    public static void main(String[] args) {
        try {
            if (args.length != 2) throw new IllegalArgumentException("Expected subscription ID and usage setting");
            int subId = Integer.parseInt(args[0]);
            int usage = Integer.parseInt(args[1]);
            if (subId < 0 || usage < 0 || usage > 2) throw new IllegalArgumentException("Invalid arguments");
            Class<?> vm = Class.forName("dalvik.system.VMRuntime");
            Object runtime = vm.getDeclaredMethod("getRuntime").invoke(null);
            vm.getDeclaredMethod("setHiddenApiExemptions", String[].class)
                .invoke(runtime, (Object) new String[]{"Landroid/", "Lcom/android/internal/"});
            if (usage == 2) {
                Class<?> resources = Class.forName("android.content.res.Resources");
                Object system = resources.getMethod("getSystem").invoke(null);
                int id = (Integer) resources.getMethod("getIdentifier", String.class, String.class, String.class)
                    .invoke(system, "config_supported_cellular_usage_settings", "array", "android");
                int[] supported = (int[]) resources.getMethod("getIntArray", int.class).invoke(system, id);
                boolean available = false;
                for (int value : supported) if (value == 2) available = true;
                if (!available) throw new IllegalStateException("Device does not support data-centric usage");
            }
            Object binder = Class.forName("android.os.ServiceManager")
                .getMethod("getService", String.class).invoke(null, "isub");
            if (binder == null) throw new IllegalStateException("Subscription service unavailable");
            Object service = Class.forName("com.android.internal.telephony.ISub$Stub")
                .getMethod("asInterface", Class.forName("android.os.IBinder")).invoke(null, binder);
            int result = (Integer) Class.forName("com.android.internal.telephony.ISub")
                .getMethod("setUsageSetting", int.class, int.class, String.class)
                .invoke(service, usage, subId, "com.android.shell");
            if (result != 1) throw new IllegalStateException("Usage setting was not accepted: " + result);
            System.out.println("Usage setting accepted");
        } catch (Exception e) {
            Throwable cause = e instanceof InvocationTargetException ? e.getCause() : e;
            System.err.println("Usage setting failed: " + cause);
            System.exit(1);
        }
    }
}
