// Objective-C, UIKit, no storyboard: everything is built in code.
#import <UIKit/UIKit.h>

@interface AppDelegate : UIResponder <UIApplicationDelegate>
@property (nonatomic, strong) UIWindow *window;
@end

@implementation AppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];

    UIViewController *root = [[UIViewController alloc] init];
    root.view.backgroundColor = UIColor.systemBackgroundColor;

    UILabel *label = [[UILabel alloc] init];
    label.text = @"Hello from darwinforge";
    label.font = [UIFont preferredFontForTextStyle:UIFontTextStyleTitle1];
    label.textAlignment = NSTextAlignmentCenter;
    label.translatesAutoresizingMaskIntoConstraints = NO;
    [root.view addSubview:label];

    [NSLayoutConstraint activateConstraints:@[
        [label.centerXAnchor constraintEqualToAnchor:root.view.centerXAnchor],
        [label.centerYAnchor constraintEqualToAnchor:root.view.centerYAnchor],
    ]];

    self.window.rootViewController = root;
    [self.window makeKeyAndVisible];
    return YES;
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil,
                                 NSStringFromClass([AppDelegate class]));
    }
}