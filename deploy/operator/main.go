// Command gateway-operator is the Kubernetes controller for the LLMGateway CRD.
//
// It is deliberately a SEPARATE binary in a SEPARATE toolchain (Go) from the
// two product binaries (gatewayd + gatewayctl, both Rust). See deploy/README.md
// "Two-binary budget": the product budget is gatewayd + gatewayctl; the
// operator is deploy/ops tooling and is kept physically outside the Rust
// workspace so it can never bloat the product's dependency tree or blur the
// product/ops boundary. It ships and versions independently.
package main

import (
	"flag"
	"os"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	metricsserver "sigs.k8s.io/controller-runtime/pkg/metrics/server"

	gwv1 "github.com/Lyqed/opensourcegateway/deploy/operator/api/v1alpha1"
	"github.com/Lyqed/opensourcegateway/deploy/operator/controllers"
)

var scheme = runtime.NewScheme()

func init() {
	utilruntime.Must(clientgoscheme.AddToScheme(scheme))
	utilruntime.Must(appsv1.AddToScheme(scheme))
	utilruntime.Must(corev1.AddToScheme(scheme))
	utilruntime.Must(gwv1.AddToScheme(scheme))
}

func main() {
	var metricsAddr, probeAddr, gatewaydImage, gatewayctlImage string
	var enableLeaderElection bool
	flag.StringVar(&metricsAddr, "metrics-bind-address", ":8080", "metrics endpoint bind address")
	flag.StringVar(&probeAddr, "health-probe-bind-address", ":8081", "health probe bind address")
	flag.BoolVar(&enableLeaderElection, "leader-elect", false, "enable leader election for HA operators")
	flag.StringVar(&gatewaydImage, "gatewayd-image", "opensourcegateway/gatewayd:smoke", "default gatewayd data-plane image")
	flag.StringVar(&gatewayctlImage, "gatewayctl-image", "opensourcegateway/gatewayctl:smoke", "default gatewayctl control-plane image")
	opts := zap.Options{Development: true}
	opts.BindFlags(flag.CommandLine)
	flag.Parse()

	ctrl.SetLogger(zap.New(zap.UseFlagOptions(&opts)))
	setupLog := ctrl.Log.WithName("setup")

	mgr, err := ctrl.NewManager(ctrl.GetConfigOrDie(), ctrl.Options{
		Scheme:                 scheme,
		Metrics:                metricsserver.Options{BindAddress: metricsAddr},
		HealthProbeBindAddress: probeAddr,
		LeaderElection:         enableLeaderElection,
		LeaderElectionID:       "gateway-operator.opensourcegateway.io",
	})
	if err != nil {
		setupLog.Error(err, "unable to start manager")
		os.Exit(1)
	}

	if err = (&controllers.Reconciler{
		Client:                 mgr.GetClient(),
		Scheme:                 mgr.GetScheme(),
		DefaultGatewaydImage:   gatewaydImage,
		DefaultGatewayctlImage: gatewayctlImage,
	}).SetupWithManager(mgr); err != nil {
		setupLog.Error(err, "unable to create controller", "controller", "LLMGateway")
		os.Exit(1)
	}

	if err := mgr.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up health check")
		os.Exit(1)
	}
	if err := mgr.AddReadyzCheck("readyz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up ready check")
		os.Exit(1)
	}

	setupLog.Info("starting gateway-operator",
		"gatewaydImage", gatewaydImage, "gatewayctlImage", gatewayctlImage)
	if err := mgr.Start(ctrl.SetupSignalHandler()); err != nil {
		setupLog.Error(err, "problem running manager")
		os.Exit(1)
	}
}
